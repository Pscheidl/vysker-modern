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
    def test_health_and_thresholds(self):
        with tempfile.TemporaryDirectory() as root, temporary_database() as database:
            root=Path(root)
            with connect(database) as db:
                db.execute('CREATE TABLE mail_queue(created_at INTEGER,attempts INTEGER,sent_at INTEGER,cancelled BOOLEAN)')
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
