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

class Monitoring(unittest.TestCase):
    def test_no_backups_keeps_application_database_disk_and_mail_checks(self):
        with tempfile.TemporaryDirectory() as root, temporary_database() as database:
            root = Path(root)
            with connect(database) as db:
                db.execute('CREATE TABLE mail_queue(created_at BIGINT,attempts BIGINT,sent_at BIGINT,cancelled BOOLEAN)')
                db.execute('CREATE TABLE recovery_mail(LIKE mail_queue)')
                db.execute('ALTER TABLE recovery_mail ADD delivery_expired_at BIGINT')
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
                db.execute('CREATE TABLE mail_queue(created_at INTEGER,attempts INTEGER,sent_at INTEGER,cancelled BOOLEAN)')
                db.execute('CREATE TABLE recovery_mail(LIKE mail_queue)')
                db.execute('ALTER TABLE recovery_mail ADD delivery_expired_at BIGINT')
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
                db.execute('CREATE TABLE mail_queue(created_at BIGINT,attempts BIGINT,sent_at BIGINT,cancelled BOOLEAN)')
                db.execute('CREATE TABLE recovery_mail(LIKE mail_queue)')
                db.execute('ALTER TABLE recovery_mail ADD delivery_expired_at BIGINT')
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
