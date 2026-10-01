"""Synchronization on disposable PostgreSQL databases, with no live web requests."""
import copy
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import sys
import tempfile
from threading import Barrier
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import Capture, digest, source_key
from legacy_import import import_bundle
from legacy_sync import SYNC_LOCK, acknowledge, export_reviews, run_sync
from postgres import connect, temporary_database
from test_legacy import add_page, bundle, navigation_bundle, FILE


def save(root, data):
    (Path(root) / 'manifest.json').write_text(json.dumps(data))


def replace_bytes(root, item, raw):
    sha = digest(raw)
    (Path(root) / 'objects' / sha).write_bytes(raw)
    item['capture'] = dict(item['capture'], sha256=sha, size=len(raw))


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class SyncTests(unittest.TestCase):
    def setUp(self):
        self.database = temporary_database()
        self.url = self.database.__enter__()
        self.directory = tempfile.TemporaryDirectory()
        self.root = self.directory.name
        with connect(self.url) as conn:
            for migration in sorted((Path(__file__).resolve().parents[1] / 'migrations').glob('*.sql')):
                conn.execute(migration.read_text())

    def tearDown(self):
        self.directory.cleanup()
        self.database.__exit__(None, None, None)

    def test_new_changed_missing_and_repeated_items_preserve_identity_and_local_edits(self):
        data = navigation_bundle(self.root)
        add_page(self.root, data, '<div id="stred"><h1>Historie</h1><p>Historie obce.</p></div>')
        with connect(self.url) as conn:
            import_bundle(conn, self.root, classify_navigation=True, publish_content=True)
            identities = conn.execute('SELECT * FROM legacy_sources ORDER BY source_key').fetchall()
            conn.execute("UPDATE pages SET content='Local editorial change'")
            original = conn.execute('SELECT * FROM attachments ORDER BY id').fetchall()
        # One old page and its document disappear. Their imported records must remain.
        new_page = copy.deepcopy(data['pages'].pop(1))
        new_page.update(key='ms:8888', url='https://vysker.cz/novinky/ms-8888', title='Nová stránka')
        data['pages'][1]['content'] = 'Změněná stránka'
        data['pages'].append(new_page)
        new_file = data['assets'].pop()
        new_file.update(url=FILE.replace('123', '789'), title='Nový dokument', parents=['ms:8888'])
        changed_file = data['assets'][0]
        replace_bytes(self.root, changed_file, b'%PDF-1.7\nchanged\n%%EOF')
        data['assets'].append(new_file)
        save(self.root, data)
        for attempt in range(2):
            with connect(self.url) as conn:
                report = import_bundle(conn, self.root, classify_navigation=True, publish_content=True, sync=True)
                self.assertEqual(len(report['new']), 1 if attempt == 0 else 0)
                self.assertEqual(len(report['conflicts']), 2)
                self.assertEqual(report['pending_reviews'], 2)
                self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0], 4)
                self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 1)
                self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 2)
                self.assertEqual(conn.execute('SELECT count(*) FROM notices').fetchone()[0], 1)
                self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 3)
                self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sync_reviews').fetchone()[0], 2)
                self.assertEqual(conn.execute('SELECT * FROM attachments ORDER BY id LIMIT 2').fetchall(), original)
                self.assertEqual(conn.execute('SELECT * FROM legacy_sources WHERE source_key=ANY(%s) ORDER BY source_key',
                    ([row[0] for row in identities],)).fetchall(), identities)
                self.assertEqual(conn.execute('SELECT content FROM pages ORDER BY id LIMIT 2').fetchall(),
                    [('Local editorial change',)])
                self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)
                self.assertEqual(conn.execute('SELECT status FROM notices').fetchone()[0], 'draft')

    def test_ownership_change_is_queued_once_even_if_content_is_unchanged(self):
        data = navigation_bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, classify_navigation=True)
        data['assets'][0]['parents'] = [data['pages'][1]['key']]
        save(self.root, data)
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, classify_navigation=True, sync=True)
            key = source_key(FILE)
            self.assertEqual(report['conflicts'], [key])
            self.assertNotIn(key, report['unchanged'])
            self.assertEqual(conn.execute('SELECT proposal->\'notice_owner\' FROM legacy_sync_reviews').fetchone()[0], False)
            self.assertEqual(conn.execute('SELECT count(*) FROM notices').fetchone()[0], 1)

    def test_changed_attachment_description_is_reviewed_without_changing_file_bytes(self):
        data = bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root)
        data['assets'][0]['evidence'] += ' Opravená příloha.'
        save(self.root, data)
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, sync=True)
            self.assertEqual(report['conflicts'], [source_key(FILE)])
            self.assertEqual(report['pending_reviews'], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 1)

    def test_review_acknowledgement_is_bound_to_exact_change_and_survives_missing_source(self):
        data = bundle(self.root, document_links=False)
        key = data['pages'][0]['key']
        with connect(self.url) as conn:
            import_bundle(conn, self.root)
        data['pages'][0]['content'] = 'First change'
        save(self.root, data)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, sync=True)
            signature = conn.execute('SELECT fingerprint FROM legacy_sync_reviews').fetchone()[0]
            acknowledge(conn, key, signature)
            self.assertEqual(import_bundle(conn, self.root, sync=True)['pending_reviews'], 0)
        data['pages'][0]['content'] = 'Second change'
        save(self.root, data)
        with connect(self.url) as conn:
            self.assertEqual(import_bundle(conn, self.root, sync=True)['pending_reviews'], 1)
            with self.assertRaisesRegex(ValueError, 'has changed'):
                acknowledge(conn, key, signature)
        data.update(pages=[], assets=[])
        save(self.root, data)
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, sync=True)
            self.assertEqual(report['pending_reviews'], 1)
            self.assertEqual(report['new'], [])
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 1)

    def test_renamed_url_and_local_deletion_never_create_another_identity(self):
        data = bundle(self.root)
        data['pages'][0]['event'] = dict(starts_at='2026-10-01T10:00:00+02:00', ends_at='2026-10-01T11:00:00+02:00',
            start_time_known=True, end_time_known=True, end_date_known=True, location='Náves')
        save(self.root, data)
        with connect(self.url) as conn:
            import_bundle(conn, self.root)
            conn.execute('DELETE FROM events')
        data['pages'][0]['url'] = 'https://www.vysker.cz/novy-nazev/ms-1053'
        save(self.root, data)
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, sync=True)
            self.assertEqual(report['new'], [])
            self.assertEqual(conn.execute('SELECT count(*) FROM events').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0], 2)
            self.assertEqual(report['pending_reviews'], 1)

    def test_concurrent_imports_insert_each_item_once(self):
        bundle(self.root)
        barrier = Barrier(2)
        def import_once():
            with connect(self.url) as conn:
                barrier.wait(timeout=10)
                return import_bundle(conn, self.root, sync=True)
        with ThreadPoolExecutor(max_workers=2) as executor:
            futures = [executor.submit(import_once) for _ in range(2)]
            reports = [future.result(timeout=20) for future in futures]
        self.assertEqual(sorted(len(report['new']) for report in reports), [0, 1])
        with connect(self.url) as conn:
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 0)

    def test_sync_discovers_new_documents_on_previously_skipped_pages(self):
        data = bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, sync=True)
        second = copy.deepcopy(data['assets'][0])
        second.update(url=FILE.replace('123', '456'), title='Nový zápis')
        data['assets'].append(second)
        data['pages'][0]['assets'].append(dict(url=second['url']))
        data['pages'][0]['content'] += f'\n[Nový zápis](<{second["url"]}>)'
        save(self.root, data)
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, sync=True)
            self.assertEqual(report['new'], [source_key(second['url'])])
            self.assertEqual(report['pending_reviews'], 0)
            self.assertEqual(len(report['skipped_document_pages']), 1)
            repeated = import_bundle(conn, self.root, sync=True)
            self.assertEqual(repeated['new'], [])
            self.assertEqual(len(repeated['unchanged']), 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 2)

    def test_sync_is_atomic_and_corrupt_source_does_not_change_live_records(self):
        data = bundle(self.root, document_links=False)
        with connect(self.url) as conn:
            import_bundle(conn, self.root)
        data['pages'][0]['content'] = 'Changed'
        new_page = dict(data['pages'][0], key='ms:8888', url='https://vysker.cz/other/ms-8888')
        data['pages'].append(new_page)
        save(self.root, data)
        with self.assertRaisesRegex(RuntimeError, 'rollback'), connect(self.url) as conn:
            report = import_bundle(conn, self.root, sync=True)
            self.assertEqual(report['pending_reviews'], 1)
            raise RuntimeError('rollback')
        with connect(self.url) as conn:
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0], 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sync_reviews').fetchone()[0], 0)
        (Path(self.root) / 'objects' / data['assets'][0]['capture']['sha256']).write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'checksum'), connect(self.url) as conn:
            import_bundle(conn, self.root, sync=True)

    def test_runner_uses_fresh_capture_and_releases_temporary_storage(self):
        roots = []
        def crawl(capture, *_):
            roots.append(capture.root)
            self.assertEqual(list(capture.root.iterdir()), [])
            data = bundle(capture.root, document_links=False)
            if len(roots) > 1:
                data['pages'][0]['content'] = 'Updated source'
                save(capture.root, data)
            return data
        with connect(self.url, autocommit=True) as conn, patch.object(Capture, 'crawl', crawl):
            for attempt in range(2):
                report = run_sync(conn, self.root, publish_content=True)
                self.assertEqual(report['status'], 'ok')
                self.assertEqual(len(report['new']), 2 if attempt == 0 else 0)
                self.assertEqual(report['pending_reviews'], attempt)
        self.assertNotEqual(roots[0], roots[1])
        self.assertFalse(any(path.exists() for path in roots))
        self.assertEqual(json.loads((Path(self.root) / 'last-success.json').read_text())['pending_reviews'], 1)
        self.assertEqual(len(json.loads((Path(self.root) / 'reviews.json').read_text())), 1)

    def test_runner_skips_overlapping_crawl(self):
        with connect(self.url, autocommit=True) as first, connect(self.url, autocommit=True) as second:
            first.execute(f'SELECT pg_advisory_lock({SYNC_LOCK})')
            with patch.object(Capture, 'crawl') as crawl:
                self.assertEqual(run_sync(second, self.root)['status'], 'skipped')
                crawl.assert_not_called()

    def test_failed_capture_keeps_last_success_and_retry_imports_only_once(self):
        def crawl(capture, *_):
            data = bundle(capture.root)
            data.update(complete=False, errors=[{'url': '/missing', 'error': '404'}])
            save(capture.root, data)
            return data
        success = Path(self.root) / 'last-success.json'
        success.write_text('{"status":"previous success"}')
        with connect(self.url, autocommit=True) as conn, patch.object(Capture, 'crawl', crawl):
            with self.assertRaisesRegex(ValueError, 'incomplete'):
                run_sync(conn, self.root)
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0], 0)
            self.assertEqual(json.loads(success.read_text())['status'], 'previous success')
            self.assertEqual(json.loads((Path(self.root) / 'last-run.json').read_text())['status'], 'failed')
            # An export failure after commit must also be safe to retry.
            with patch('legacy_sync.export_reviews', side_effect=OSError('disk full')):
                with self.assertRaises(OSError):
                    run_sync(conn, self.root, allow_incomplete=True)
            self.assertTrue(json.loads((Path(self.root) / 'last-run.json').read_text())['import_committed'])
            report = run_sync(conn, self.root, allow_incomplete=True)
            self.assertEqual(report['new'], [])
            self.assertEqual(len(report['capture_errors']), 1)

    def test_review_export_preserves_proposed_file_and_escapes_source_content(self):
        data = bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root)
        raw = b'%PDF-1.7\nnew bytes\n%%EOF'
        filename = 'ž' * 170 + ' #1.pdf'
        data['assets'][0].update(title='<script>alert(1)</script>', name=filename)
        replace_bytes(self.root, data['assets'][0], raw)
        save(self.root, data)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, sync=True)
        # The queue remains reviewable after the offline capture disappears.
        for file in (Path(self.root) / 'objects').iterdir():
            file.unlink()
        with tempfile.TemporaryDirectory() as output, connect(self.url) as conn:
            self.assertEqual(export_reviews(conn, output, 'https://new.example.test'), 1)
            html = (Path(output) / 'reviews.html').read_text()
            self.assertNotIn('<script>', html)
            self.assertIn('&lt;script&gt;', html)
            self.assertIn(f'download="{filename}"', html)
            self.assertIn('https://new.example.test/api/v1/attachments/', html)
            records = json.loads((Path(output) / 'reviews.json').read_text())
            self.assertEqual((Path(output) / records[0]['proposed_file']).read_bytes(), raw)
            self.assertLess(len(Path(records[0]['proposed_file']).name.encode()), 255)
            with self.assertRaisesRegex(ValueError, 'HTTP'):
                export_reviews(conn, output, 'javascript:alert(1)')


if __name__ == '__main__':
    unittest.main()
