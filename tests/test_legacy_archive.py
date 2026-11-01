"""Historical availability decisions preserve canonical content and never mail history."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy_archive import archive_imported_notices, plan_archive
from legacy_import import import_bundle
from legacy_documents import configure_source
from postgres import connect, temporary_database
from test_legacy import bundle, subscription_fixture

WRITE_LOCK = "hashtextextended(current_schema() || ':vysker-write', 0)"


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class ArchiveTests(unittest.TestCase):
    def setUp(self):
        self.database = self.enterContext(temporary_database())
        self.directory = self.enterContext(tempfile.TemporaryDirectory())
        self.data = bundle(self.directory, notice=True)
        with connect(self.database) as conn:
            for migration in sorted((Path(__file__).resolve().parents[1] / 'migrations').glob('*.sql')):
                conn.execute(migration.read_text())
            configure_source(conn, 'one_time', allow_publication=True, reason='Anonymous archive fixture')

    def imported(self, conn):
        import_bundle(conn, self.directory)
        return conn.execute('SELECT id FROM documents').fetchone()[0]

    def test_read_only_plan_and_archive_preserve_content_files_and_source_dates(self):
        with connect(self.database) as conn:
            subscription_fixture(conn)
            document = self.imported(conn)
            before = conn.execute('SELECT title,description,content_sha256,source_published_on,lock_state FROM documents').fetchone()
            files = conn.execute('SELECT * FROM attachments').fetchall()
            dates = conn.execute('SELECT started_on,started_at,removed_on,removed_at FROM document_postings').fetchall()
        with connect(self.database) as conn:
            conn.execute('SET TRANSACTION READ ONLY')
            self.assertEqual(plan_archive(conn)['candidate_ids'], [document])
        with connect(self.database) as conn:
            self.assertEqual(archive_imported_notices(conn, 1)['archived_ids'], [document])
            self.assertEqual(conn.execute('SELECT title,description,content_sha256,source_published_on,lock_state FROM documents').fetchone(), before)
            self.assertEqual(conn.execute('SELECT * FROM attachments').fetchall(), files)
            self.assertEqual(conn.execute('SELECT started_on,started_at,removed_on,removed_at FROM document_postings').fetchall(), dates)
            self.assertTrue(conn.execute('SELECT public_access FROM documents').fetchone()[0])
            self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)

    def test_unknown_dates_and_local_withdrawal_are_excluded(self):
        self.data['pages'][0]['dates'] = {}
        (Path(self.directory) / 'manifest.json').write_text(json.dumps(self.data))
        with connect(self.database) as conn:
            self.imported(conn)
            report = plan_archive(conn)
            self.assertEqual(report['candidate_count'], 0)
            self.assertIn('incomplete_dates', report['excluded'][0]['reasons'])
            self.assertEqual(conn.execute('SELECT lock_state,public_access FROM documents').fetchone(), ('review', False))
            conn.execute('UPDATE documents SET access_blocked=TRUE')
            self.assertIn('locally_blocked', plan_archive(conn)['excluded'][0]['reasons'])

    def test_source_policy_and_expected_count_guard_leave_content_unchanged(self):
        with connect(self.database) as conn:
            self.imported(conn)
            with self.assertRaisesRegex(ValueError, 'Expected 2.*found 1'):
                archive_imported_notices(conn, 2)
            configure_source(conn, 'proposals', reason='Disable source publication for review')
            with self.assertRaisesRegex(ValueError, 'does not allow'):
                archive_imported_notices(conn, 1)
            self.assertFalse(conn.execute('SELECT public_access FROM documents').fetchone()[0])

    def test_idempotence_and_standard_write_lock(self):
        with connect(self.database) as conn:
            self.imported(conn)
        with connect(self.database) as conn, connect(self.database, autocommit=True) as concurrent:
            archive_imported_notices(conn, 1)
            self.assertFalse(concurrent.execute(f'SELECT pg_try_advisory_lock({WRITE_LOCK})').fetchone()[0])
        with connect(self.database) as conn:
            subscription_fixture(conn)
            self.assertEqual(archive_imported_notices(conn, 0)['archived_count'], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 0)

    def test_failed_conversion_rolls_back_availability_and_audit(self):
        with connect(self.database) as conn:
            self.imported(conn)
            before = conn.execute('SELECT * FROM audit_log ORDER BY id').fetchall()
        with self.assertRaisesRegex(RuntimeError, 'rollback'), connect(self.database) as conn:
            archive_imported_notices(conn, 1)
            raise RuntimeError('rollback')
        with connect(self.database) as conn:
            self.assertFalse(conn.execute('SELECT public_access FROM documents').fetchone()[0])
            self.assertEqual(conn.execute('SELECT * FROM audit_log ORDER BY id').fetchall(), before)


if __name__ == '__main__':
    unittest.main()
