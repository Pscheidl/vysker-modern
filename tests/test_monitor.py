import importlib.util
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))
from postgres import connect, temporary_database
import tempfile
import unittest
from unittest.mock import patch, MagicMock

spec=importlib.util.spec_from_file_location('monitor',Path(__file__).parents[1]/'scripts/monitor.py')
monitor=importlib.util.module_from_spec(spec)
spec.loader.exec_module(monitor)

def mail_tables(db):
    db.execute('CREATE TABLE mail_queue(created_at BIGINT,attempts BIGINT,sent_at BIGINT,cancelled BOOLEAN)')
    db.execute('CREATE TABLE subscribers(id BIGINT PRIMARY KEY,next_digest_at BIGINT NOT NULL DEFAULT 0)')
    db.execute('INSERT INTO subscribers(id) VALUES (1)')
    db.execute('CREATE TABLE publication_outbox(created_at BIGINT,subscriber_id BIGINT NOT NULL DEFAULT 1,consent_id BIGINT NOT NULL DEFAULT 1)')
    db.execute('CREATE TABLE recovery_mail(LIKE mail_queue)')
    db.execute('ALTER TABLE recovery_mail ADD delivery_expired_at BIGINT')


class Monitoring(unittest.TestCase):
    def test_no_backups_keeps_application_database_disk_and_mail_checks(self):
        with tempfile.TemporaryDirectory() as root, temporary_database() as database:
            root = Path(root)
            with connect(database) as db:
                mail_tables(db)
                db.execute('INSERT INTO recovery_mail VALUES (1,4,NULL,FALSE,NULL)')
            usage = type('Usage', (), dict(total=100000, free=50000))()
            with patch.object(monitor.urllib.request, 'urlopen', side_effect=OSError()), \
                    patch.object(monitor.shutil, 'disk_usage', return_value=usage) as disk:
                report = monitor.check(database, root/'absent', 'http://localhost',
                                       now=10000, database_disk=root, check_backups=False)
            disk.assert_called_once_with(root)
            self.assertEqual(report['alerts'], ['application_not_ready', 'database_disk_space_low',
                                               'mail_queue_stalled', 'recovery_mail_stalled'])
            self.assertNotIn('backup_age_seconds', report['metrics'])
            self.assertEqual(report['metrics']['pending_mail'], 1)
            with patch.object(monitor.urllib.request, 'urlopen', side_effect=OSError()):
                report = monitor.check('postgresql://invalid@127.0.0.1:1/missing', root/'absent',
                                       'http://localhost', check_backups=False)
            self.assertIn('database_check_failed', report['alerts'])
            self.assertNotIn('backup_missing', report['alerts'])

    def test_backup_optout_is_explicit_and_cli_can_enable_it_again(self):
        report = {'checked_at': 10000, 'status': 'ok', 'alerts': [], 'metrics': {}}
        for setting, options, expected in [
                ('true', [], True), ('false', [], False),
                ('false', ['--check-backups'], True), ('true', ['--no-check-backups'], False)]:
            with patch.dict(monitor.os.environ, {'OBEC_MONITOR_CHECK_BACKUPS': setting}), \
                    patch.object(sys, 'argv', ['monitor.py', 'check', *options]), \
                    patch.object(monitor, 'check', return_value=report) as check, \
                    patch('builtins.print'):
                self.assertEqual(monitor.main(), 0)
                self.assertIs(check.call_args.kwargs['check_backups'], expected)
        with patch.dict(monitor.os.environ, {'OBEC_MONITOR_CHECK_BACKUPS': 'maybe'}), \
                patch.object(sys, 'argv', ['monitor.py', 'check']), \
                patch.object(sys, 'stderr'), self.assertRaises(SystemExit) as error:
            monitor.main()
        self.assertEqual(error.exception.code, 2)

    def test_health_and_thresholds(self):
        with tempfile.TemporaryDirectory() as root, temporary_database() as database:
            root=Path(root)
            with connect(database) as db:
                mail_tables(db)
                db.execute('INSERT INTO mail_queue VALUES (1,4,NULL,FALSE)')
                db.execute('INSERT INTO mail_queue VALUES (1,99,NULL,TRUE)')
            snapshot=root/'vysker-test.dump'
            snapshot.write_bytes(b'snapshot')
            response=MagicMock()
            response.__enter__.return_value.status=200
            response.__enter__.return_value.read.return_value=b'{"status":"ready"}'
            with patch.object(monitor.urllib.request,'urlopen',return_value=response),patch.object(monitor.shutil,'disk_usage',return_value=type('Usage',(),dict(total=100000,free=50000))()):
                report=monitor.check(database,root,'http://localhost',now=snapshot.stat().st_mtime,min_free_mib=1,database_disk=root)
                self.assertIn('mail_queue_stalled',report['alerts'])
                self.assertIn('database_disk_space_low',report['alerts'])
                self.assertEqual(report['metrics']['pending_mail'],1)
                self.assertEqual(report['metrics']['maximum_mail_attempts'],4)
                self.assertNotIn('backup_stale',report['alerts'])
                report=monitor.check(database,root,'http://localhost',now=snapshot.stat().st_mtime+31*3600)
                self.assertIn('backup_stale',report['alerts'])
            with patch.object(monitor.urllib.request,'urlopen',side_effect=OSError()):
                report=monitor.check('postgresql://invalid@127.0.0.1:1/missing',root/'absent','http://localhost')
                self.assertIn('application_not_ready',report['alerts'])
                self.assertIn('backup_missing',report['alerts'])
                self.assertIn('database_check_failed',report['alerts'])

    def test_recovery_failure_is_visible_before_and_after_expiry(self):
        with tempfile.TemporaryDirectory() as root, temporary_database() as database:
            with connect(database) as db:
                mail_tables(db)
                db.execute('INSERT INTO recovery_mail VALUES (9000,3,NULL,FALSE,NULL)')
            with patch.object(monitor.urllib.request,'urlopen',side_effect=OSError()):
                report=monitor.check(database,root,'http://localhost',now=10000)
                self.assertIn('recovery_mail_stalled',report['alerts'])
                with connect(database) as db:
                    db.execute('UPDATE recovery_mail SET cancelled=TRUE,delivery_expired_at=10800')
                report=monitor.check(database,root,'http://localhost',now=11000)
                self.assertIn('recovery_delivery_expired',report['alerts'])
                self.assertEqual(report['metrics']['expired_recovery_last_day'],1)
                with connect(database) as db:
                    db.execute('UPDATE recovery_mail SET delivery_expired_at=NULL')
                report=monitor.check(database,root,'http://localhost',now=11000)
                self.assertNotIn('recovery_delivery_expired',report['alerts'])

    def test_publication_handoff_is_monitored_before_and_after_materialization(self):
        with temporary_database() as database:
            with connect(database) as db:
                mail_tables(db)
                db.execute('INSERT INTO publication_outbox(created_at) VALUES (0)')
            response = MagicMock()
            response.__enter__.return_value.status = 200
            response.__enter__.return_value.read.return_value = b'{"status":"ready"}'
            with patch.object(monitor.urllib.request, 'urlopen', return_value=response):
                waiting = monitor.check(database, '/unused', 'http://localhost', now=10000,
                                        check_backups=False, max_pending=1)
                self.assertEqual(waiting['alerts'], ['mail_queue_large', 'mail_queue_stalled'])
                self.assertEqual(waiting['metrics']['pending_mail'], 1)
                self.assertEqual(waiting['metrics']['oldest_mail_age_seconds'], 10000)
                self.assertEqual(waiting['metrics']['oldest_mail_overdue_seconds'], 6400)
                self.assertEqual(waiting['metrics']['maximum_mail_attempts'], 0)
                # The worker atomically exchanges handoff records for rendered mail.
                with connect(database) as db:
                    db.execute('INSERT INTO mail_queue SELECT 10000,2,NULL,FALSE FROM publication_outbox')
                    db.execute('DELETE FROM publication_outbox')
                ready = monitor.check(database, '/unused', 'http://localhost', now=10000,
                                      check_backups=False, max_pending=1)
                self.assertEqual(ready['alerts'], ['mail_queue_large'])
                self.assertEqual(ready['metrics']['pending_mail'], 1)
                self.assertEqual(ready['metrics']['oldest_mail_age_seconds'], 0)
                self.assertEqual(ready['metrics']['oldest_mail_overdue_seconds'], 0)
                self.assertEqual(ready['metrics']['maximum_mail_attempts'], 2)
                retrying = monitor.check(database, '/unused', 'http://localhost', now=11801,
                                         check_backups=False, max_pending=1)
                self.assertEqual(retrying['alerts'], ['mail_queue_large', 'mail_queue_stalled'])

    def test_hourly_waiting_is_not_a_backlog_and_uses_the_subscriber_next_slot(self):
        with temporary_database() as database:
            with connect(database) as db:
                mail_tables(db)
                db.execute('INSERT INTO publication_outbox(created_at) VALUES (3601),(3602)')
            response = MagicMock()
            response.__enter__.return_value.status = 200
            response.__enter__.return_value.read.return_value = b'{"status":"ready"}'
            with patch.object(monitor.urllib.request, 'urlopen', return_value=response):
                def check(now):
                    return monitor.check(database, '/unused', 'http://localhost', now=now,
                                         check_backups=False, max_pending=1)

                waiting = check(7199)
                self.assertEqual(waiting['alerts'], [])
                self.assertEqual(waiting['metrics']['pending_mail'], 2)
                self.assertEqual(waiting['metrics']['scheduled_publications'], 2)
                self.assertEqual(waiting['metrics']['ready_mail'], 0)
                self.assertEqual(waiting['metrics']['oldest_mail_overdue_seconds'], 0)
                self.assertEqual(check(7200)['alerts'], ['mail_queue_large'])
                self.assertEqual(check(9000)['alerts'], ['mail_queue_large'])
                self.assertEqual(check(9001)['alerts'], ['mail_queue_large', 'mail_queue_stalled'])

                # A late publication from the same hour waits until the next
                # digest slot when the subscriber already received a digest.
                with connect(database) as db:
                    db.execute('UPDATE subscribers SET next_digest_at=10800')
                self.assertEqual(check(9001)['alerts'], [])
                self.assertEqual(check(10799)['metrics']['scheduled_publications'], 2)
                due = check(10800)
                self.assertEqual(due['alerts'], ['mail_queue_large'])
                self.assertEqual(due['metrics']['scheduled_publications'], 0)
                self.assertEqual(due['metrics']['ready_mail'], 1)
                self.assertEqual(check(12601)['alerts'], ['mail_queue_large', 'mail_queue_stalled'])

    def test_ready_backlog_counts_digest_groups_instead_of_publication_items(self):
        with temporary_database() as database:
            with connect(database) as db:
                mail_tables(db)
                db.execute('INSERT INTO subscribers(id) SELECT generate_series(2,10)')
                db.execute('''INSERT INTO publication_outbox(created_at,subscriber_id,consent_id)
                    SELECT 3601,s.id,s.id+100 FROM subscribers s CROSS JOIN generate_series(1,200)''')
            response = MagicMock()
            response.__enter__.return_value.status = 200
            response.__enter__.return_value.read.return_value = b'{"status":"ready"}'
            with patch.object(monitor.urllib.request, 'urlopen', return_value=response):
                report = monitor.check(database, '/unused', 'http://localhost', now=7200,
                                       check_backups=False, max_pending=11)
                self.assertEqual(report['alerts'], [])
                self.assertEqual(report['metrics']['pending_mail'], 2000)
                self.assertEqual(report['metrics']['scheduled_publications'], 0)
                self.assertEqual(report['metrics']['ready_mail'], 10)
                crowded = monitor.check(database, '/unused', 'http://localhost', now=7200,
                                        check_backups=False, max_pending=10)
                self.assertEqual(crowded['alerts'], ['mail_queue_large'])

                with connect(database) as db:
                    # A different original consent remains a separate group.
                    db.execute('INSERT INTO publication_outbox(created_at,consent_id) VALUES (3601,201)')
                    db.execute('INSERT INTO mail_queue VALUES (7200,0,NULL,FALSE),(7200,1,7200,FALSE),(7200,1,NULL,TRUE)')
                    db.execute('INSERT INTO recovery_mail VALUES (7200,0,NULL,FALSE,NULL)')
                combined = monitor.check(database, '/unused', 'http://localhost', now=7200,
                                         check_backups=False, max_pending=13)
                self.assertEqual(combined['alerts'], ['mail_queue_large'])
                self.assertEqual(combined['metrics']['pending_mail'], 2003)
                self.assertEqual(combined['metrics']['ready_mail'], 13)

    def test_scheduled_publications_do_not_hide_stalled_verification_mail(self):
        with temporary_database() as database:
            with connect(database) as db:
                mail_tables(db)
                db.execute('INSERT INTO publication_outbox(created_at) VALUES (3601)')
                db.execute('INSERT INTO mail_queue VALUES (3601,1,NULL,FALSE)')
            response = MagicMock()
            response.__enter__.return_value.status = 200
            response.__enter__.return_value.read.return_value = b'{"status":"ready"}'
            with patch.object(monitor.urllib.request, 'urlopen', return_value=response):
                report = monitor.check(database, '/unused', 'http://localhost', now=7199,
                                       check_backups=False)
            self.assertEqual(report['alerts'], ['mail_queue_stalled'])
            self.assertEqual(report['metrics']['scheduled_publications'], 1)
            self.assertEqual(report['metrics']['ready_mail'], 1)
            self.assertEqual(report['metrics']['oldest_mail_overdue_seconds'], 3598)

    def test_notifications_are_deduplicated_and_recovery_is_sent(self):
        report={'alerts':['backup_stale'],'checked_at':10000}
        self.assertTrue(monitor.notification_due(report,{}))
        previous={'notified_alerts':['backup_stale'],'notified_at':9999}
        self.assertFalse(monitor.notification_due(report,previous))
        self.assertTrue(monitor.notification_due({**report,'checked_at':14000},previous))
        self.assertTrue(monitor.notification_due({'alerts':[],'checked_at':10001},previous))
        self.assertFalse(monitor.notification_due({'alerts':[],'checked_at':10001},{}))
        with self.assertRaises(ValueError): monitor.send_webhook('http://insecure.test',report)

    def test_state_is_atomic_and_private(self):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'state.json'
            monitor.write_state(path,{'status':'ok'})
            self.assertEqual(json.loads(path.read_text()),{'status':'ok'})
            self.assertEqual(path.stat().st_mode & 0o777,0o600)
            monitor.write_state(path,{'status':'alert'})
            self.assertEqual(json.loads(path.read_text()),{'status':'alert'})

if __name__=='__main__': unittest.main()
