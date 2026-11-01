"""Retired destructive import paths cannot bypass canonical document preservation."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy_import import import_bundle
from legacy_replace import replace_import, require_staging
from legacy_reimport import reset_import, reset_library
from postgres import connect, temporary_database
from test_legacy import bundle, LOCAL_PREVIEW_ENV, subscription_fixture


class StagingGuardTests(unittest.TestCase):
    def test_nonproduction_must_be_explicit(self):
        for value in ('', 'true'):
            with self.subTest(value=value), patch.dict(os.environ, {'OBEC_PRODUCTION': value}):
                with self.assertRaises(ValueError):
                    require_staging()
        with patch.dict(os.environ, {'OBEC_PRODUCTION': 'false'}):
            require_staging()


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class RetiredReplacementTests(unittest.TestCase):
    def setUp(self):
        self.database = self.enterContext(temporary_database())
        self.directory = self.enterContext(tempfile.TemporaryDirectory())
        bundle(self.directory, notice=True)
        with connect(self.database) as conn:
            for migration in sorted((Path(__file__).resolve().parents[1] / 'migrations').glob('*.sql')):
                conn.execute(migration.read_text())
            subscription_fixture(conn)
            import_bundle(conn, self.directory, archive_notices=True)

    def test_staging_replacement_refuses_before_any_content_or_mail_change(self):
        with patch.dict(os.environ, LOCAL_PREVIEW_ENV), connect(self.database) as conn:
            tables = ('documents', 'attachments', 'legacy_sources', 'document_postings', 'publication_outbox', 'mail_queue')
            before = {table: conn.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall() for table in tables}
            count = conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0]
            with self.assertRaisesRegex(ValueError, 'Destructive reimport is disabled'):
                replace_import(conn, self.directory, count, archive_notices=True, skip_pages=True)
            self.assertEqual(before, {table: conn.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall() for table in tables})
            self.assertEqual(conn.execute("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled='D'").fetchone()[0], 0)

    def test_local_replacement_cannot_unlock_or_reuse_document_identity(self):
        with patch.dict(os.environ, LOCAL_PREVIEW_ENV), connect(self.database) as conn:
            for action in (reset_library, reset_import):
                with self.subTest(action=action.__name__), self.assertRaisesRegex(ValueError, 'Destructive reimport is disabled'):
                    action(conn)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT lock_state FROM documents').fetchone()[0], 'locked')


if __name__ == '__main__':
    unittest.main()
