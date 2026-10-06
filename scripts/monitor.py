#!/usr/bin/env python3
"""Read-only operational checks and optional deduplicated JSON webhook alerts."""
import argparse
import json
import os
from pathlib import Path
import shutil
import psycopg
from postgres import connect, database_url
import tempfile
import time
import urllib.request
import urllib.parse


def check(database, backups, ready_url, *, now=None, backup_hours=30,
          mail_minutes=30, recovery_minutes=10, max_pending=1000, min_free_mib=1024, min_free_percent=10, database_disk=None, check_backups=True):
    now = time.time() if now is None else now
    alerts, metrics = [], {}
    try:
        with urllib.request.urlopen(ready_url, timeout=5) as response:
            if response.status != 200 or json.load(response).get('status') != 'ready':
                raise ValueError('not ready')
    except Exception:
        alerts.append('application_not_ready')
    try:
        # Only inspect the database disk when its actual volume is mounted here.
        if database_disk:
            usage = shutil.disk_usage(database_disk)
            metrics['free_mib'] = usage.free // 1048576
            metrics['free_percent'] = round(100 * usage.free / usage.total, 1)
            if metrics['free_mib'] < min_free_mib or metrics['free_percent'] < min_free_percent:
                alerts.append('database_disk_space_low')
        if check_backups:
            usage = shutil.disk_usage(backups)
            if usage.free / 1048576 < min_free_mib or 100 * usage.free / usage.total < min_free_percent:
                alerts.append('backup_disk_space_low')
    except OSError:
        alerts.append('disk_check_failed')
    if check_backups:
        try:
            snapshots = [p for p in Path(backups).glob('vysker-*.dump') if p.is_file() and not p.is_symlink() and p.stat().st_size > 0]
            newest = max(p.stat().st_mtime for p in snapshots)
            age = now - newest
            metrics['backup_age_seconds'] = int(age)
            if age < -300 or age > backup_hours * 3600:
                alerts.append('backup_stale')
        except (OSError, ValueError):
            alerts.append('backup_missing')
    try:
        with connect(database, options='-c default_transaction_read_only=on') as conn:
            pending, oldest, retries = conn.execute('''SELECT count(*),min(created_at),coalesce(max(attempts),0)
                FROM (SELECT created_at,attempts FROM mail_queue WHERE sent_at IS NULL AND cancelled=FALSE
                    UNION ALL SELECT created_at,0 FROM publication_outbox
                    UNION ALL SELECT created_at,attempts FROM recovery_mail WHERE sent_at IS NULL AND cancelled=FALSE) pending''').fetchone()
            recovery_oldest = conn.execute('SELECT min(created_at) FROM recovery_mail WHERE sent_at IS NULL AND cancelled=FALSE').fetchone()[0]
            recovery_expired = conn.execute('SELECT count(*) FROM recovery_mail WHERE sent_at IS NULL AND delivery_expired_at>=%s', (int(now)-86400,)).fetchone()[0]
        metrics['expired_recovery_last_day'] = recovery_expired
        if recovery_oldest is not None and now-recovery_oldest > recovery_minutes*60:
            alerts.append('recovery_mail_stalled')
        if recovery_expired:
            alerts.append('recovery_delivery_expired')
        metrics.update(pending_mail=pending, oldest_mail_age_seconds=max(0, int(now-oldest)) if oldest is not None else 0, maximum_mail_attempts=retries)
        if pending >= max_pending:
            alerts.append('mail_queue_large')
        if oldest is not None and now - oldest > mail_minutes * 60:
            alerts.append('mail_queue_stalled')
    except (OSError, psycopg.Error):
        alerts.append('database_check_failed')
    return {'checked_at': int(now), 'status': 'alert' if alerts else 'ok', 'alerts': sorted(alerts), 'metrics': metrics}


def write_state(path, state):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix='.monitor-')
    try:
        with os.fdopen(fd, 'w') as handle:
            json.dump(state, handle)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def notification_due(report, previous, repeat_seconds=3600):
    # Notify on first failure, changed failures, hourly reminders and recovery.
    before = previous.get('notified_alerts', [])
    return report['alerts'] != before or bool(report['alerts'] and report['checked_at'] - previous.get('notified_at', 0) >= repeat_seconds)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def send_webhook(url, report):
    if urllib.parse.urlsplit(url).scheme != 'https':
        raise ValueError('Alert webhook must use HTTPS')
    request = urllib.request.Request(url, data=json.dumps(report).encode(), headers={'Content-Type': 'application/json'}, method='POST')
    with urllib.request.build_opener(NoRedirect).open(request, timeout=10) as response:
        if not 200 <= response.status < 300:
            raise RuntimeError('Webhook rejected notification')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['check', 'daemon', 'health'])
    parser.add_argument('--database')
    parser.add_argument('--database-disk', help='Mount of the actual PostgreSQL volume, omitted for managed databases')
    parser.add_argument('--backups', default='/app/backups')
    backup_setting = os.environ.get('OBEC_MONITOR_CHECK_BACKUPS', 'true')
    if backup_setting not in ('true', 'false'):
        parser.error('OBEC_MONITOR_CHECK_BACKUPS must be true or false')
    parser.add_argument('--check-backups', action=argparse.BooleanOptionalAction,
                        default=backup_setting == 'true',
                        help='Check backup freshness and disk space, enabled by default')
    parser.add_argument('--ready-url', default='http://web:3000/api/v1/ready')
    parser.add_argument('--state', default='/app/monitor/state.json')
    parser.add_argument('--interval', type=int, default=60)
    parser.add_argument('--backup-hours', type=int, default=30)
    parser.add_argument('--mail-minutes', type=int, default=30)
    parser.add_argument('--recovery-minutes', type=int, default=10)
    parser.add_argument('--max-pending', type=int, default=1000)
    parser.add_argument('--min-free-mib', type=int, default=1024)
    parser.add_argument('--min-free-percent', type=int, default=10)
    args = parser.parse_args()
    if min(args.interval,args.backup_hours,args.mail_minutes,args.recovery_minutes,args.max_pending,args.min_free_mib) <= 0 or not 1<=args.min_free_percent<=99:
        parser.error('Thresholds must be positive, disk percentage must be 1 to 99')
    if args.mode == 'health':
        try:
            state = json.loads(Path(args.state).read_text())
            return 0 if 0 <= time.time()-state['checked_at'] < args.interval*3 and state['status']=='ok' and not state.get('notification_failed') else 1
        except (OSError, ValueError, KeyError):
            return 1
    while True:
        report = check(database_url(args.database),args.backups,args.ready_url,backup_hours=args.backup_hours,mail_minutes=args.mail_minutes,recovery_minutes=args.recovery_minutes,max_pending=args.max_pending,min_free_mib=args.min_free_mib,min_free_percent=args.min_free_percent,database_disk=args.database_disk,check_backups=args.check_backups)
        if args.mode == 'check':
            print(json.dumps(report),flush=True)
            return 0 if report['status']=='ok' else 2
        try:
            previous = json.loads(Path(args.state).read_text())
        except (OSError, ValueError):
            previous = {}
        state = {**previous, **report, 'notification_failed': False}
        if notification_due(report,previous):
            print(json.dumps(report),flush=True)
            secret_file = os.environ.get('OBEC_ALERT_WEBHOOK_FILE')
            try:
                if secret_file:
                    send_webhook(Path(secret_file).read_text().strip(), report)
                state.update(notified_alerts=report['alerts'],notified_at=report['checked_at'])
            except Exception:
                # Neither the secret URL nor HTTP response bodies belong in logs.
                state['notification_failed'] = True
                print(json.dumps({'event':'alert_delivery_failed'}),flush=True)
        write_state(args.state,state)
        time.sleep(args.interval)


if __name__ == '__main__':
    raise SystemExit(main())
