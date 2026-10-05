"""Explicit staging replacement preserves local data and rolls back failed imports."""
import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import digest, event_timing, source_key
from legacy_import import import_bundle
from legacy_reimport import reset_import
from legacy_replace import replace_import, require_staging
from postgres import connect, temporary_database
from test_legacy import add_page, navigation_bundle, FILE


STAGING = {'OBEC_PRODUCTION': 'false', 'OBEC_VEREJNA_URL': 'https://staging.example.test'}
TABLES = ('pages', 'page_revisions', 'page_images', 'documents', 'notices', 'events',
          'attachments', 'notice_events', 'legacy_sources', 'legacy_notice_imports',
          'legacy_sync_reviews', 'audit_log', 'administrators', 'subscribers', 'mail_settings', 'mail_queue')


class StagingGuardTests(unittest.TestCase):
    def test_nonproduction_must_be_explicit(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, 'OBEC_PRODUCTION=false'):
                require_staging()
            for value in ('true', '1', 'no', ''):
                with patch.dict(os.environ, {'OBEC_PRODUCTION': value}):
                    with self.assertRaisesRegex(ValueError, 'OBEC_PRODUCTION=false'):
                        require_staging()
            with patch.dict(os.environ, STAGING):
                require_staging()


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class ReplaceTests(unittest.TestCase):
    def setUp(self):
        self.database = temporary_database()
        self.url = self.database.__enter__()
        self.directory = tempfile.TemporaryDirectory()
        self.root = self.directory.name
        self.environment = patch.dict(os.environ, STAGING)
        self.environment.start()
        with connect(self.url) as conn:
            for migration in sorted((Path(__file__).resolve().parents[1] / 'migrations').glob('*.sql')):
                conn.execute(migration.read_text())
        self.data = navigation_bundle(self.root)
        add_page(self.root, self.data, '<div id="stred"><h1>Historie</h1><p>Původní text.</p></div>')
        event = add_page(self.root, self.data, '<div id="stred"><h1>Setkání</h1><p>Na návsi.</p></div>',
                         'https://vysker.cz/setkani/a-1')
        event['event'] = dict(event_timing('1.10.2026 19:00'), location='Náves')
        image = copy.deepcopy(self.data['assets'][0])
        raw = b'\x89PNG\r\n\x1a\nfixture'
        sha = digest(raw)
        (Path(self.root) / 'objects' / sha).write_bytes(raw)
        image.update(url='https://vysker.cz/assets/Image.ashx?id_obrazky=10',
                     name='kaple.png', title='Kaple', mime='image/png', parents=['ms:7777'],
                     capture=dict(image['capture'], sha256=sha, size=len(raw)))
        self.data['assets'].append(image)
        (Path(self.root) / 'manifest.json').write_text(json.dumps(self.data))
        with connect(self.url) as conn:
            import_bundle(conn, self.root, classify_navigation=True, publish_content=True)
            self.source_count = conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0]

    def tearDown(self):
        self.environment.stop()
        self.directory.cleanup()
        self.database.__exit__(None, None, None)

    def snapshot(self, conn):
        return {table: conn.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall() for table in TABLES}

    def test_replacement_imports_only_documents_archived_notices_and_events(self):
        with connect(self.url) as conn:
            page_id = conn.execute("INSERT INTO pages(slug,title,content,updated_at) VALUES ('native','Native','Local text','2026-10-01') RETURNING id").fetchone()[0]
            doc_id = conn.execute("INSERT INTO documents(title,created_at) VALUES ('Native','2026-10-01') RETURNING id").fetchone()[0]
            conn.execute("INSERT INTO notices(title) VALUES ('Native')")
            conn.execute("INSERT INTO events(title,starts_at,ends_at,updated_at) VALUES ('Native','2026-10-01','2026-10-02','2026-10-01')")
            conn.execute("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,'native.pdf','application/pdf',1,%s)", (doc_id, b'x'))
            conn.execute('''INSERT INTO page_revisions(page_id,version,title,content,slug,published,saved_at)
                SELECT id,version,title,content,slug,published,updated_at FROM pages WHERE id=%s''', (page_id,))
            admin_id = conn.execute("INSERT INTO administrators(email,password_hash) VALUES ('native@example.test','hash') RETURNING id").fetchone()[0]
            subscriber_id = conn.execute("INSERT INTO subscribers(email,retention_started_at) VALUES ('subscriber@example.test',1) RETURNING id").fetchone()[0]
            conn.execute("INSERT INTO mail_settings VALUES (1,'native@example.test','Native',%s,1,%s)", (b'ciphertext', admin_id))
            conn.execute("INSERT INTO mail_queue(subscriber_id,purpose,subject,next_attempt_at,created_at) VALUES (%s,'verification','Existing',1,1)", (subscriber_id,))
            protected = {table: conn.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall()
                         for table in ('administrators', 'subscribers', 'mail_settings', 'mail_queue', 'audit_log')}
            native = {table: conn.execute('SELECT * FROM ' + table + " WHERE title='Native' ORDER BY id").fetchall()
                      for table in ('pages', 'documents', 'notices', 'events')}
            ids = dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL'))
            original_max = {table: conn.execute('SELECT max(id) FROM ' + table).fetchone()[0]
                            for table in ('documents', 'notices', 'events')}
        with connect(self.url) as conn:
            result = replace_import(conn, self.root, self.source_count, archive_notices=True, skip_pages=True)
            self.assertFalse(result['backup_created'])
            self.assertEqual(result['reset_counts']['pages'], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources WHERE page_id IS NOT NULL').fetchone()[0], 0)
            self.assertEqual(conn.execute("SELECT count(*) FROM attachments WHERE content_type LIKE 'image/%'").fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources WHERE event_id IS NOT NULL').fetchone()[0], 1)
            self.assertEqual(conn.execute("SELECT n.status,n.retain_attachments,n.published_at,n.withdrawn_at FROM notices n JOIN legacy_sources s ON s.notice_id=n.id").fetchall(), [('archived', True, None, None)])
            self.assertEqual(conn.execute("SELECT d.status FROM documents d JOIN legacy_sources s ON s.document_id=d.id").fetchall(), [('published',)])
            self.assertEqual(conn.execute('SELECT count(*) FROM notice_events').fetchone()[0], 0)
            for table, rows in native.items():
                self.assertEqual(conn.execute('SELECT * FROM ' + table + " WHERE title='Native' ORDER BY id").fetchall(), rows)
            for table, rows in protected.items():
                current = conn.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall()
                self.assertEqual(current[:len(rows)] if table == 'audit_log' else current, rows)
            surviving = dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL'))
            self.assertEqual(surviving, {key: ids[key] for key in (source_key(FILE), source_key(FILE.replace('123', '456')))})
            for table, previous in original_max.items():
                self.assertGreater(conn.execute('SELECT max(id) FROM ' + table).fetchone()[0], previous)
            self.assertEqual(conn.execute("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled='D'").fetchone()[0], 0)

    def test_guards_leave_all_records_unchanged_and_local_reset_stays_local(self):
        with connect(self.url) as conn:
            before = self.snapshot(conn)
            with self.assertRaisesRegex(ValueError, 'Expected .* imported sources'):
                replace_import(conn, self.root, self.source_count + 1, archive_notices=True, skip_pages=True)
            with self.assertRaisesRegex(ValueError, '--archive-notices and --skip-pages'):
                replace_import(conn, self.root, self.source_count, archive_notices=False, skip_pages=True)
            with self.assertRaisesRegex(ValueError, '--archive-notices and --skip-pages'):
                replace_import(conn, self.root, self.source_count, archive_notices=True, skip_pages=False)
            with patch.dict(os.environ, {'OBEC_PRODUCTION': 'true'}):
                with self.assertRaisesRegex(ValueError, 'OBEC_PRODUCTION=false'):
                    replace_import(conn, self.root, self.source_count, archive_notices=True, skip_pages=True)
            with self.assertRaisesRegex(ValueError, 'local'):
                reset_import(conn)
            self.assertEqual(self.snapshot(conn), before)

    def test_corrupt_or_incomplete_bundle_never_starts_reset(self):
        with connect(self.url) as conn:
            before = self.snapshot(conn)
            self.data['complete'] = False
            (Path(self.root) / 'manifest.json').write_text(json.dumps(self.data))
            with patch('legacy_replace.reset_imported_content') as reset:
                with self.assertRaisesRegex(ValueError, 'incomplete'):
                    replace_import(conn, self.root, self.source_count, archive_notices=True, skip_pages=True)
                reset.assert_not_called()
            self.data['complete'] = True
            (Path(self.root) / 'manifest.json').write_text(json.dumps(self.data))
            (Path(self.root) / 'objects' / self.data['assets'][0]['capture']['sha256']).write_bytes(b'corrupt')
            with patch('legacy_replace.reset_imported_content') as reset:
                with self.assertRaisesRegex(ValueError, 'checksum'):
                    replace_import(conn, self.root, self.source_count, archive_notices=True, skip_pages=True)
                reset.assert_not_called()
            self.assertEqual(self.snapshot(conn), before)

    def test_failure_after_reset_automatically_rolls_back_even_when_caller_catches_it(self):
        with connect(self.url) as conn:
            before = self.snapshot(conn)
            with patch('legacy_replace.import_bundle', side_effect=RuntimeError('forced import failure')):
                with self.assertRaisesRegex(RuntimeError, 'forced import failure'):
                    replace_import(conn, self.root, self.source_count, archive_notices=True, skip_pages=True)
            self.assertEqual(self.snapshot(conn), before)
            self.assertEqual(conn.execute("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled='D'").fetchone()[0], 0)

    def test_repeated_replacement_preserves_download_ids_without_duplicates(self):
        with connect(self.url) as conn:
            first = replace_import(conn, self.root, self.source_count, archive_notices=True, skip_pages=True)
            sources = conn.execute('SELECT source_key,attachment_id FROM legacy_sources ORDER BY source_key').fetchall()
            counts = {table: conn.execute('SELECT count(*) FROM ' + table).fetchone()[0]
                      for table in ('pages', 'events', 'documents', 'notices', 'attachments', 'mail_queue')}
            second = replace_import(conn, self.root, first['resulting_source_count'], archive_notices=True, skip_pages=True)
            self.assertEqual(second['resulting_source_count'], first['resulting_source_count'])
            self.assertEqual(conn.execute('SELECT source_key,attachment_id FROM legacy_sources ORDER BY source_key').fetchall(), sources)
            self.assertEqual({table: conn.execute('SELECT count(*) FROM ' + table).fetchone()[0] for table in counts}, counts)
            repeated_sync = import_bundle(conn, self.root, publish_content=True, classify_navigation=True,
                                          archive_notices=True, skip_pages=True, sync=True)
            self.assertEqual(repeated_sync['new'], [])
            self.assertEqual(repeated_sync['conflicts'], [])


if __name__ == '__main__':
    unittest.main()
