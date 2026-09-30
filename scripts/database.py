#!/usr/bin/env python3
"""Consistent PostgreSQL backups and sanitized restore into a NEW database."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from postgres import connect, create_database, database_url, drop_database, run_tool


def copy_database(source, destination):
    destination = Path(destination).absolute()
    if destination.exists() or destination.is_symlink():
        raise RuntimeError('Destination already exists, refusing to overwrite it')
    destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix='.snapshot-', dir=destination.parent)
    os.close(fd)
    try:
        run_tool('pg_dump', source, '--format=custom', '--no-owner', '--no-acl', '--file', temporary)
        subprocess.run(['pg_restore', '--list', temporary], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        with open(temporary, 'rb') as handle:
            os.fsync(handle.fileno())
        os.link(temporary, destination)
        directory = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        Path(temporary).unlink(missing_ok=True)


def restore_database(source, destination):
    source = Path(source).resolve(strict=True)
    # CREATE DATABASE refuses existing destinations, including the live database.
    create_database(destination)
    try:
        run_tool('pg_restore', destination, '--single-transaction', '--exit-on-error', '--no-owner', '--no-acl', str(source))
        with connect(destination) as conn:
            # A stale backup must never reactivate a subscription withdrawn later.
            conn.execute('DELETE FROM subscribers')
            conn.execute('DELETE FROM sessions')
            # Backups from the PostgreSQL baseline predate recovery tables.
            if conn.execute("SELECT to_regclass('password_resets') IS NOT NULL").fetchone()[0]:
                conn.execute('DELETE FROM password_resets')
            if conn.execute("SELECT to_regclass('recovery_mail') IS NOT NULL").fetchone()[0]:
                conn.execute('DELETE FROM recovery_mail')
            conn.execute('DELETE FROM rate_limits')
            conn.execute("INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id) VALUES (%s,'restored','database',1)",
                         (dt.datetime.now(dt.timezone.utc).isoformat().replace('+00:00', 'Z'),))
    except BaseException:
        drop_database(destination)
        raise


def backup(database, directory, keep_days):
    if not 1 <= keep_days <= 3650:
        raise ValueError('keep-days must be between 1 and 3650')
    directory = Path(directory)
    stamp = dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
    result = directory / f'vysker-{stamp}.dump'
    copy_database(database, result)
    cutoff = time.time() - keep_days * 86400
    for old in directory.glob('vysker-*.dump'):
        if old != result and not old.is_symlink() and old.is_file() and old.stat().st_mtime < cutoff:
            old.unlink()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    save = commands.add_parser('backup')
    save.add_argument('--database', help='Defaults to OBEC_DATABAZE or OBEC_DATABAZE_FILE')
    save.add_argument('--directory', required=True)
    save.add_argument('--keep-days', required=True, type=int)
    daemon = commands.add_parser('daemon')
    daemon.add_argument('--database')
    daemon.add_argument('--directory', required=True)
    daemon.add_argument('--privacy-config', required=True)
    restore = commands.add_parser('restore')
    restore.add_argument('--source', required=True)
    restore.add_argument('--destination', help='Connection string for a NEW database, defaults to OBEC_DATABAZE_FILE')
    restore.add_argument('--discard-subscriptions', action='store_true', required=True,
                         help='Required: users must opt in again after recovery')
    args = parser.parse_args()
    if args.command == 'daemon':
        while True:
            policy = json.loads(Path(args.privacy_config).read_text())
            print(backup(database_url(args.database), args.directory, int(policy['retention']['backup_days'])), flush=True)
            time.sleep(86400)
    elif args.command == 'backup':
        print(backup(database_url(args.database), args.directory, args.keep_days))
    else:
        restore_database(args.source, database_url(args.destination))
        print('Restored into a new PostgreSQL database. Subscriptions and sessions have been removed.')


if __name__ == '__main__':
    main()
