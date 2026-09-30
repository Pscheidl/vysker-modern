"""Real PostgreSQL backup/restore tests on databases created and removed by this suite."""
from pathlib import Path
import secrets
import sys
import tempfile
import unittest
sys.path.insert(0, str(Path(__file__).parents[1] / 'scripts'))
import database
from postgres import connect, drop_database, renamed_database, temporary_database


class DatabaseTest(unittest.TestCase):
    def test_backup_no_overwrite_and_safe_restore(self):
        with tempfile.TemporaryDirectory() as temp, temporary_database() as source:
            root = Path(temp)
            with connect(source) as live:
                live.execute((Path(__file__).parents[1] / 'migrations/0001_initial.sql').read_text())
                live.execute("INSERT INTO subscribers(email,retention_started_at) VALUES ('example@example.test',1)")
                live.execute("INSERT INTO mail_queue(subscriber_id,purpose,subject,created_at,next_attempt_at) VALUES (1,'verification','Verify',1,1)")
                live.execute("INSERT INTO administrators(email,password_hash) VALUES ('admin@example.test','hash')")
                live.execute("INSERT INTO sessions VALUES ('session','csrf',1,9999999999)")
                live.execute("INSERT INTO notices(title,published_on) VALUES ('Dokument','2026-09-30')")
            snapshot = database.backup(source, root / 'backups', 30)
            self.assertEqual(snapshot.stat().st_mode & 0o777, 0o600)
            self.assertTrue(snapshot.read_bytes().startswith(b'PGDMP'))
            with self.assertRaises(RuntimeError):
                database.copy_database(source, snapshot)
            with self.assertRaises(Exception):
                database.restore_database(snapshot, source)
            restored = renamed_database(source, 'vysker_restore_' + secrets.token_hex(12))
            try:
                database.restore_database(snapshot, restored)
                with connect(restored) as restored_db:
                    for table in ('subscribers', 'mail_queue', 'sessions'):
                        self.assertEqual(restored_db.execute(f'SELECT count(*) FROM {table}').fetchone()[0], 0)
                    self.assertEqual(restored_db.execute('SELECT title FROM notices').fetchone()[0], 'Dokument')
                    self.assertEqual(restored_db.execute('SELECT operation FROM audit_log').fetchone()[0], 'restored')
                with connect(source) as live:
                    self.assertEqual(live.execute('SELECT count(*) FROM subscribers').fetchone()[0], 1)
            finally:
                drop_database(restored)


if __name__ == '__main__':
    unittest.main()
