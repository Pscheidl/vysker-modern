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
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import Capture, ORIGIN, digest, is_asset, source_key
from legacy_import import import_bundle
from legacy_sync import SYNC_LOCK, acknowledge, export_reviews, imported_asset_loader, run_sync
from postgres import connect, temporary_database
from test_legacy import add_page, bundle, navigation_bundle, subscription_fixture, FILE, HTML, URL
from test_legacy_capture import Response


def save(root, data):
    (Path(root) / 'manifest.json').write_text(json.dumps(data))


def replace_bytes(root, item, raw):
    sha = digest(raw)
    (Path(root) / 'objects' / sha).write_bytes(raw)
    item['capture'] = dict(item['capture'], sha256=sha, size=len(raw))


class ImportedAssetLoaderTests(unittest.TestCase):
    def test_invalid_provenance_hash_is_rejected_before_querying_attachment_bytes(self):
        for invalid in (None, 123, {}, [], 'A' * 64, '0' * 63, '../object'):
            with self.subTest(sha256=invalid):
                conn = Mock()
                conn.execute.return_value.fetchall.return_value = [
                    (FILE, dict(url=FILE, size=10, sha256=invalid), 1)]
                self.assertIsNone(imported_asset_loader(conn)(FILE))
                self.assertEqual(conn.execute.call_count, 1)


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
        with connect(self.url) as conn:
            subscription_fixture(conn)
        barrier = Barrier(2)
        def import_once():
            with connect(self.url) as conn:
                barrier.wait(timeout=10)
                return import_bundle(conn, self.root, sync=True, publish_content=True)
        with ThreadPoolExecutor(max_workers=2) as executor:
            futures = [executor.submit(import_once) for _ in range(2)]
            reports = [future.result(timeout=20) for future in futures]
        self.assertEqual(sorted(len(report['new']) for report in reports), [0, 1])
        with connect(self.url) as conn:
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 2)

    def test_import_publication_uses_subscriber_filters_for_notices_and_documents(self):
        navigation_bundle(self.root)
        with connect(self.url) as conn:
            subscribers = subscription_fixture(conn)
            category = conn.execute("SELECT id FROM categories WHERE name='Ostatní'").fetchone()[0]
            # Imported notice categories and general documents are independent choices.
            conn.execute('''UPDATE subscribers SET all_notice_categories=FALSE,
                notice_category_ids=%s,uncategorized_notices=FALSE,documents=FALSE WHERE id=%s''',
                ([category], subscribers['active'][0]))
            conn.execute('''UPDATE subscribers SET all_notice_categories=FALSE,
                uncategorized_notices=FALSE,documents=TRUE WHERE id=%s''',
                (subscribers['active_other'][0],))
            import_bundle(conn, self.root, classify_navigation=True, publish_content=True,
                          archive_notices=True, sync=True)
            notice = conn.execute('SELECT id FROM notices').fetchone()[0]
            document = conn.execute('SELECT id FROM documents').fetchone()[0]
            self.assertEqual(conn.execute('''SELECT notice_id,document_id,subscriber_id,consent_id
                FROM publication_outbox ORDER BY subscriber_id''').fetchall(), [
                (notice, None, *subscribers['active']),
                (None, document, *subscribers['active_other']),
            ])
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)

    def test_sync_discovers_new_documents_on_previously_skipped_pages(self):
        data = bundle(self.root)
        with connect(self.url) as conn:
            subscribers = subscription_fixture(conn)
            import_bundle(conn, self.root, sync=True)
        second = copy.deepcopy(data['assets'][0])
        second.update(url=FILE.replace('123', '456'), title='Nový zápis')
        data['assets'].append(second)
        data['pages'][0]['assets'].append(dict(url=second['url']))
        data['pages'][0]['content'] += f'\n[Nový zápis](<{second["url"]}>)'
        save(self.root, data)
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, sync=True, publish_content=True)
            self.assertEqual(report['new'], [source_key(second['url'])])
            self.assertEqual(report['pending_reviews'], 0)
            self.assertEqual(len(report['skipped_document_pages']), 1)
            repeated = import_bundle(conn, self.root, sync=True, publish_content=True)
            self.assertEqual(repeated['new'], [])
            self.assertEqual(len(repeated['unchanged']), 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 2)
            published_id = conn.execute("SELECT id FROM documents WHERE status='published'").fetchone()[0]
            self.assertEqual(conn.execute('''SELECT notice_id,document_id,subscriber_id,consent_id
                FROM publication_outbox ORDER BY id''').fetchall(),
                [(None, published_id, *subscribers[name]) for name in ('active', 'active_other')])

    def test_skip_pages_ignores_source_page_conflicts_and_reviews_without_changing_local_pages(self):
        data = bundle(self.root, document_links=False)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, publish_content=True)
            conn.execute("UPDATE pages SET content='Místní oprava kontaktů'")
            before = conn.execute('SELECT * FROM pages').fetchall()
        page = data['pages'][0]
        page['content'] = 'Nový text původního webu'
        page['warnings'] = ['A source-page warning that is outside the selected scope']
        page['review_required'] = 'A source-page review that is outside the selected scope'
        save(self.root, data)
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, sync=True, skip_pages=True)
            self.assertEqual(report['new'], [])
            self.assertEqual(report['conflicts'], [])
            self.assertEqual(report['review'], [])
            self.assertEqual(report['pending_reviews'], 0)
            self.assertEqual(report['skipped_content'][0]['key'], page['key'])
            self.assertTrue(report['skipped_content'][0]['already_imported'])
            self.assertEqual(conn.execute('SELECT * FROM pages').fetchall(), before)

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

    def test_runner_bootstraps_existing_files_then_downloads_only_new_attachments(self):
        html = [HTML.encode()]
        requested = []
        original_bytes = b'%PDF-1.7\nfixture\n%%EOF'
        raw = [original_bytes]
        second = FILE.replace('123', '456')

        def open_response(request, timeout):
            url = request.full_url
            requested.append(url)
            if is_asset(url):
                return Response(url, raw[0] if url == FILE else original_bytes,
                    {'Content-Type': 'application/pdf', 'Content-Disposition': 'attachment; filename="zapis.pdf"'})
            if url.endswith('/vismo/sitemap.asp'):
                body = ('<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
                        f'<url><loc>{URL}</loc></url></urlset>').encode()
            else:
                body = html[0] if url == URL else b'<div id="stred"><h1>Navigation</h1></div>'
            return Response(url, body, {'Content-Type': 'text/html'})

        with patch('urllib.request.OpenerDirector.open', side_effect=open_response):
            initial = Path(self.root) / 'initial'
            Capture(initial, 0, verbose=False).crawl()
            with connect(self.url) as conn:
                import_bundle(conn, initial, publish_content=True, skip_pages=True)
                original = conn.execute('SELECT * FROM attachments').fetchall()
                metadata = conn.execute("SELECT metadata->'capture' FROM legacy_sources").fetchone()[0]
            requested.clear()
            state = Path(self.root) / 'sync'
            with connect(self.url, autocommit=True) as conn:
                first = run_sync(conn, state, delay=0, publish_content=True, skip_pages=True)
                self.assertEqual(first['status'], 'ok')
                self.assertEqual(first['new'], [])
                self.assertEqual(first['conflicts'], [])
                self.assertEqual(first['download_stats']['assets_seeded'], 1)
                self.assertEqual(first['download_stats']['assets_reused'], 1)
                self.assertEqual(first['download_stats']['assets_downloaded'], 0)
                self.assertFalse(any(is_asset(url) for url in requested))
                self.assertIn(URL, requested)
                cached = state / 'capture-cache' / 'responses' / (digest(FILE.encode()) + '.json')
                self.assertEqual(json.loads(cached.read_text()), metadata)

                # New links on an existing HTML page are discovered on the next run.
                html[0] = HTML.replace('</ul>', f'<li><a href="{second}">Nový dokument</a></li></ul>').encode()
                requested.clear()
                following = run_sync(conn, state, delay=0, publish_content=True, skip_pages=True)
                self.assertEqual(following['new'], [source_key(second)])
                self.assertEqual(following['conflicts'], [])
                self.assertEqual(following['download_stats']['assets_seeded'], 0)
                self.assertEqual(following['download_stats']['assets_reused'], 1)
                self.assertEqual(following['download_stats']['assets_downloaded'], 1)
                self.assertEqual([url for url in requested if is_asset(url)], [second])
                self.assertEqual(conn.execute('SELECT * FROM attachments ORDER BY id LIMIT 1').fetchall(), original)
                self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)
                self.assertGreater(following['duration_seconds'], 0)

                # An explicit refresh detects a replacement at the same URL, preserving
                # the imported original and queuing the new bytes for editorial review.
                raw[0] = b'%PDF-1.7\nreplacement\n%%EOF'
                refreshed = run_sync(conn, state, delay=0, publish_content=True,
                    skip_pages=True, refresh_assets=True)
                self.assertEqual(refreshed['conflicts'], [source_key(FILE)])
                self.assertEqual(refreshed['download_stats']['assets_downloaded'], 2)
                self.assertEqual(refreshed['download_stats']['assets_reused'], 0)
                self.assertEqual(conn.execute('SELECT data FROM attachments ORDER BY id LIMIT 1').fetchone()[0], original_bytes)
                # A queued replacement stays in cache without another HTTP download.
                repeated = run_sync(conn, state, delay=0, publish_content=True, skip_pages=True)
                self.assertEqual(repeated['download_stats']['assets_reused'], 2)
                self.assertEqual(repeated['download_stats']['assets_downloaded'], 0)
                self.assertEqual(repeated['pending_reviews'], 1)
                self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)

    def test_database_asset_loader_rejects_changed_corrupt_or_removed_bytes(self):
        data = bundle(self.root)
        item = data['assets'][0]
        item['capture'].update(url=FILE, final_url=FILE, content_type='application/pdf',
            disposition='attachment; filename="zapis.pdf"')
        save(self.root, data)
        with connect(self.url, autocommit=True) as conn:
            with conn.transaction():
                import_bundle(conn, self.root)
            load = imported_asset_loader(conn)
            original = load(FILE)
            self.assertIsNotNone(original)
            self.assertEqual(original[0], item['capture'])
            self.assertIsNone(load(FILE.replace('123', '456')))
            for column, value in [('data', b'corrupt'), ('sha256', '0' * 64),
                                  ('size_bytes', 1), ('content_type', 'image/png')]:
                with self.subTest(column=column):
                    with conn.transaction(force_rollback=True):
                        conn.execute(f'UPDATE attachments SET {column}=%s', (value,))
                        self.assertIsNone(load(FILE))
            conn.execute("UPDATE attachments SET data=NULL, removed_at='2026-10-05'")
            self.assertIsNone(load(FILE))
            self.assertIsNone(imported_asset_loader(conn)(FILE))

    def test_failed_import_keeps_verified_cache_for_retry_without_duplicate_or_email(self):
        incomplete = [True]
        raw = b'%PDF-1.7\nfixture\n%%EOF'

        def crawl(capture, *_):
            data = bundle(capture.root)
            metadata, _ = capture.fetch(FILE, 30 * 1024 * 1024)
            data['assets'][0]['capture'] = metadata
            if incomplete[0]:
                data.update(complete=False, errors=[{'url': URL, 'error': 'HTTP 503'}])
            save(capture.root, data)
            return data

        with connect(self.url, autocommit=True) as conn, patch.object(Capture, 'crawl', crawl), \
                patch('urllib.request.OpenerDirector.open', return_value=Response(FILE, raw,
                    {'Content-Type': 'application/pdf', 'Content-Disposition': 'attachment; filename="zapis.pdf"'})) as http:
            with self.assertRaisesRegex(ValueError, 'incomplete'):
                run_sync(conn, self.root, delay=0, publish_content=True, skip_pages=True)
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0], 0)
            failed = json.loads((Path(self.root) / 'last-run.json').read_text())
            self.assertEqual(failed['download_stats']['assets_downloaded'], 1)
            incomplete[0] = False
            for attempt in range(2):
                report = run_sync(conn, self.root, delay=0, publish_content=True, skip_pages=True)
                self.assertEqual(len(report['new']), 1 if attempt == 0 else 0)
                self.assertEqual(report['download_stats']['assets_reused'], 1)
                self.assertEqual(report['download_stats']['assets_downloaded'], 0)
                self.assertEqual(report['conflicts'], [])
            self.assertEqual(http.call_count, 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)

    def test_runner_archives_only_new_imported_notices_and_preserves_native_private_records(self):
        def crawl(capture, *_):
            data = navigation_bundle(capture.root)
            add_page(capture.root, data, '<div id="stred"><h1>Kontakt</h1><p>Vynechaná stránka.</p></div>')
            return data
        with connect(self.url, autocommit=True) as conn, patch.object(Capture, 'crawl', crawl):
            subscribers = subscription_fixture(conn)
            conn.execute("INSERT INTO notices(title,status,published_on) VALUES ('Místní koncept','draft',NULL),('Místní plán','scheduled','2030-03-02')")
            native = conn.execute('SELECT * FROM notices ORDER BY id').fetchall()
            for attempt in range(2):
                report = run_sync(conn, self.root, publish_content=True, archive_notices=True, skip_pages=True)
                self.assertEqual(report['status'], 'ok')
                self.assertEqual(len(report['new']), 2 if attempt == 0 else 0)
                self.assertTrue(report['skip_pages'])
                self.assertEqual(len(report['skipped_content']), 3)
                self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
                self.assertEqual(conn.execute('SELECT * FROM notices WHERE title LIKE %s ORDER BY id', ('Místní%',)).fetchall(), native)
                self.assertEqual(conn.execute("SELECT status,retain_attachments,published_at,withdrawn_at FROM notices WHERE title='Zápis'").fetchone(),
                    ('archived', True, None, None))
                self.assertEqual(conn.execute('SELECT count(*) FROM attachments WHERE data IS NOT NULL').fetchone()[0], 2)
                self.assertEqual(conn.execute('SELECT count(*) FROM notice_events').fetchone()[0], 0)
                self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)
                self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 4)
                self.assertEqual(conn.execute('''SELECT DISTINCT subscriber_id,consent_id
                    FROM publication_outbox ORDER BY subscriber_id''').fetchall(),
                    [subscribers[name] for name in ('active', 'active_other')])

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
