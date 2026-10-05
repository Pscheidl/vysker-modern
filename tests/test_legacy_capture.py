"""Offline crawler completeness regressions, without PostgreSQL or live requests."""
import json
from email.message import Message
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import Capture, ORIGIN, digest


PAGE = ORIGIN + '/source-profile/o-1010'
FILE = ORIGIN + '/assets/missing.pdf'
HTML = f'''<div id="stred"><h1>Page</h1><p>Preserved content.</p>
    <a href="{FILE}">Document</a></div>'''.encode()
SITEMAP = ('<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
           f'<url><loc>{PAGE}</loc></url></urlset>').encode()


class CaptureCompletenessTests(unittest.TestCase):
    def crawl(self, failures):
        def fetch(url, _limit):
            if url in failures:
                raise failures[url]
            if url.endswith('/vismo/sitemap.asp'):
                raw = SITEMAP
            elif url == FILE:
                raw = b'%PDF-1.7\nexample'
            else:
                raw = HTML
            return {'url': url, 'size': len(raw), 'disposition': ''}, raw

        with tempfile.TemporaryDirectory() as root:
            capture = Capture(root, delay=0, verbose=False)
            with patch.object(capture, 'fetch', side_effect=fetch):
                result = capture.crawl()
            self.assertEqual(result, json.loads((Path(root) / 'manifest.json').read_text()))
            return result

    def test_page_404_is_audited_and_does_not_block_other_content(self):
        result = self.crawl({PAGE: HTTPError(PAGE, 404, 'Not Found', {}, None)})
        self.assertTrue(result['complete'])
        self.assertEqual(result['errors'], [])
        entry, = result['unavailable_resources']
        self.assertEqual((entry['url'], entry['kind'], entry['status']), (PAGE, 'page', 404))
        self.assertTrue(entry['checked_at'])
        missing = next(page for page in result['pages'] if page['url'] == PAGE)
        self.assertIn('error', missing)
        self.assertNotIn('capture', missing)
        self.assertTrue(any('capture' in page for page in result['pages']))

    def test_attachment_404_keeps_surrounding_content_and_original_link(self):
        result = self.crawl({FILE: HTTPError(FILE, 404, 'Not Found', {}, None)})
        self.assertTrue(result['complete'])
        self.assertEqual(result['errors'], [])
        entry, = result['unavailable_resources']
        self.assertEqual((entry['url'], entry['kind'], entry['status']), (FILE, 'asset', 404))
        asset, = result['assets']
        self.assertIn('error', asset)
        self.assertNotIn('capture', asset)
        self.assertTrue(asset['parents'])
        page = next(page for page in result['pages'] if page['url'] == PAGE)
        self.assertIn('Preserved content.', page['content'])
        self.assertIn(FILE, page['content'])

    def test_other_http_status_and_network_failure_still_block(self):
        for url in (PAGE, FILE):
            for error in (HTTPError(url, 500, 'Server Error', {}, None),
                          HTTPError(url, 410, 'Gone', {}, None), TimeoutError('Timeout')):
                with self.subTest(url=url, error=error):
                    result = self.crawl({url: error})
                    self.assertFalse(result['complete'])
                    self.assertEqual(result['unavailable_resources'], [])
                    self.assertEqual([entry['url'] for entry in result['errors']], [url])

    def test_restored_resources_are_captured_normally(self):
        result = self.crawl({})
        self.assertTrue(result['complete'])
        self.assertEqual(result['unavailable_resources'], [])
        self.assertTrue(all('capture' in item for item in result['pages'] + result['assets']))

    def test_missing_sitemap_is_audited_and_seed_discovery_continues(self):
        url = ORIGIN + '/vismo/sitemap.asp'
        result = self.crawl({url: HTTPError(url, 404, 'Not Found', {}, None)})
        self.assertTrue(result['complete'])
        entry, = result['unavailable_resources']
        self.assertEqual((entry['url'], entry['kind'], entry['status']), (url, 'sitemap', 404))
        self.assertIn(ORIGIN + '/', [page['url'] for page in result['pages']])
        self.assertIn(FILE, [asset['url'] for asset in result['assets']])

    def test_sitemap_server_error_still_blocks_discovery(self):
        url = ORIGIN + '/vismo/sitemap.asp'
        with self.assertRaises(HTTPError):
            self.crawl({url: HTTPError(url, 500, 'Server Error', {}, None)})


class Response(io.BytesIO):
    def __init__(self, url, body, headers=None):
        super().__init__(body)
        self.url = url
        self.headers = Message()
        for name, value in (headers or {}).items():
            self.headers[name] = value


class IncrementalCaptureTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cache = self.root / 'shared'
        self.counter = 0
        self.pdf = b'%PDF-1.7\noriginal'

    def capture(self, **kwargs):
        self.counter += 1
        return Capture(self.root / str(self.counter), delay=0, verbose=False,
                       cache_root=self.cache, **kwargs)

    def prime(self, url=FILE, body=None, headers=None):
        capture = self.capture()
        raw = self.pdf if body is None else body
        capture.opener.open = Mock(return_value=Response(url, raw, headers))
        return capture.fetch(url, 100_000)

    def test_known_asset_is_reused_without_network_or_delay_and_metadata_preserved(self):
        original = self.prime(headers={'Content-Disposition': 'attachment; filename="original.pdf"'})
        capture = self.capture()
        capture.opener.open = Mock(side_effect=AssertionError('Known asset requested'))
        with patch('legacy.time.sleep') as sleep:
            result = capture.fetch(FILE, 100_000)
        self.assertEqual(result, original)
        sleep.assert_not_called()
        capture.opener.open.assert_not_called()
        self.assertEqual(capture.blob(original[0]['sha256']), self.pdf)
        self.assertEqual(capture.stats['assets_reused'], 1)
        self.assertEqual(capture.stats['network_requests'], 0)
        self.assertEqual(capture.stats['downloaded_bytes'], 0)

    def test_new_attachment_on_existing_page_is_discovered_and_only_it_downloaded(self):
        new_file = ORIGIN + '/assets/new.pdf'
        html = [HTML]
        requested = []

        def open_response(request, timeout):
            url = request.full_url
            requested.append(url)
            raw = SITEMAP if url.endswith('/sitemap.asp') else (
                self.pdf if url in (FILE, new_file) else html[0])
            return Response(url, raw)

        first = self.capture()
        first.opener.open = Mock(side_effect=open_response)
        self.assertTrue(first.crawl()['complete'])
        requested.clear()
        html[0] = HTML.replace(b'</div>', f'<a href="{new_file}">New</a></div>'.encode())
        second = self.capture()
        second.opener.open = Mock(side_effect=open_response)
        manifest = second.crawl()
        self.assertTrue(manifest['complete'])
        self.assertIn(PAGE, requested)
        self.assertNotIn(FILE, requested)
        self.assertEqual(requested.count(new_file), 1)
        self.assertEqual({item['url'] for item in manifest['assets']}, {FILE, new_file})
        self.assertEqual(second.stats['assets_reused'], 1)
        self.assertEqual(second.stats['assets_downloaded'], 1)

    def test_asset_identity_includes_query_parameters(self):
        first = ORIGIN + '/assets/File.ashx?id_dokumenty=1&id_org=18774'
        second = ORIGIN + '/assets/File.ashx?id_dokumenty=2&id_org=18774'
        self.prime(url=first)
        capture = self.capture()
        capture.opener.open = Mock(return_value=Response(second, b'%PDF-1.7\nsecond'))
        capture.fetch(second, 100_000)
        capture.opener.open.assert_called_once()
        self.assertEqual(capture.fetch(first, 100_000)[1], self.pdf)
        self.assertEqual(capture.stats['assets_downloaded'], 1)
        self.assertEqual(capture.stats['assets_reused'], 1)

    def test_corrupt_or_missing_cache_object_is_repaired_by_download(self):
        for damage in ('corrupt', 'missing'):
            with self.subTest(damage=damage):
                response, _ = self.prime()
                obj = self.cache / 'objects' / response['sha256']
                obj.write_bytes(b'corrupt') if damage == 'corrupt' else obj.unlink()
                capture = self.capture()
                capture.opener.open = Mock(return_value=Response(FILE, self.pdf))
                self.assertEqual(capture.fetch(FILE, 100_000)[1], self.pdf)
                self.assertEqual(capture.stats['cache_repairs'], 1)
                self.assertEqual(capture.stats['assets_downloaded'], 1)
                self.assertEqual(obj.read_bytes(), self.pdf)

    def test_sparse_oversized_cache_object_is_repaired_without_reading_it(self):
        for metadata_too_large in (False, True):
            with self.subTest(metadata_too_large=metadata_too_large):
                response, _ = self.prime()
                obj = self.cache / 'objects' / response['sha256']
                oversized = 10 * 1024 * 1024 * 1024
                with obj.open('wb') as output:
                    output.truncate(oversized)
                if metadata_too_large:
                    index = self.cache / 'responses' / (digest(FILE.encode()) + '.json')
                    metadata = json.loads(index.read_text())
                    metadata['size'] = oversized
                    index.write_text(json.dumps(metadata))
                original_open = Path.open

                def bounded_open(path, *args, **kwargs):
                    if path == obj:
                        self.assertLessEqual(path.stat().st_size, 100_000,
                                             'Oversized cache object must not be read')
                    return original_open(path, *args, **kwargs)

                capture = self.capture()
                capture.opener.open = Mock(return_value=Response(FILE, self.pdf))
                with patch.object(Path, 'open', bounded_open):
                    self.assertEqual(capture.fetch(FILE, 100_000)[1], self.pdf)
                self.assertEqual(capture.stats['cache_repairs'], 1)
                self.assertEqual(capture.stats['assets_downloaded'], 1)
                self.assertEqual(obj.read_bytes(), self.pdf)

    def test_invalid_asset_response_is_not_cached_and_next_run_retries(self):
        capture = self.capture()
        capture.opener.open = Mock(return_value=Response(FILE, b'<html>Temporary error</html>'))
        with self.assertRaises(ValueError):
            capture.fetch(FILE, 100_000)
        self.assertFalse((self.cache / 'responses' / (digest(FILE.encode()) + '.json')).exists())
        fresh = self.capture()
        fresh.opener.open = Mock(return_value=Response(FILE, self.pdf))
        self.assertEqual(fresh.fetch(FILE, 100_000)[1], self.pdf)
        fresh.opener.open.assert_called_once()

    def test_database_loader_seeds_persistent_cache_without_http(self):
        metadata = dict(url=FILE, final_url=FILE, captured_at='2026-10-01T12:00:00Z',
                        content_type='application/pdf', disposition='attachment; filename="known.pdf"',
                        size=len(self.pdf), sha256=digest(self.pdf))
        loader = Mock(return_value=(metadata, self.pdf))
        capture = self.capture(asset_loader=loader)
        capture.opener.open = Mock(side_effect=AssertionError('Seeded asset requested'))
        self.assertEqual(capture.fetch(FILE, 100_000), (metadata, self.pdf))
        loader.assert_called_once_with(FILE)
        self.assertEqual(capture.stats['assets_seeded'], 1)
        self.assertEqual(capture.stats['assets_reused'], 1)
        another = self.capture(asset_loader=Mock(side_effect=AssertionError('Already seeded')))
        another.opener.open = Mock(side_effect=AssertionError('Already cached'))
        self.assertEqual(another.fetch(FILE, 100_000), (metadata, self.pdf))

    def test_invalid_loader_object_falls_back_to_remote(self):
        capture = self.capture(asset_loader=Mock(return_value=({}, self.pdf)))
        capture.opener.open = Mock(return_value=Response(FILE, self.pdf))
        self.assertEqual(capture.fetch(FILE, 100_000)[1], self.pdf)
        self.assertEqual(capture.stats['assets_seeded'], 0)
        self.assertEqual(capture.stats['cache_repairs'], 1)

    def test_cache_metadata_must_match_url_size_and_authorized_origin(self):
        for field, value in [('url', ORIGIN + '/assets/other.pdf'), ('size', 1),
                             ('final_url', 'https://example.com/private.pdf'),
                             ('final_url', 123), ('url', 123), ('sha256', []),
                             ('size', '12'), ('size', True)]:
            with self.subTest(field=field):
                self.prime()
                index = self.cache / 'responses' / (digest(FILE.encode()) + '.json')
                metadata = json.loads(index.read_text())
                metadata[field] = value
                index.write_text(json.dumps(metadata))
                capture = self.capture()
                capture.opener.open = Mock(return_value=Response(FILE, self.pdf))
                capture.fetch(FILE, 100_000)
                self.assertEqual(capture.stats['cache_repairs'], 1)
                self.assertEqual(capture.stats['assets_downloaded'], 1)

    def test_oversized_response_index_is_repaired_without_reading_it(self):
        self.prime()
        index = self.cache / 'responses' / (digest(FILE.encode()) + '.json')
        with index.open('wb') as output:
            output.truncate(10 * 1024 * 1024 * 1024)
        original_open = Path.open

        def bounded_open(path, *args, **kwargs):
            if path == index:
                self.assertLessEqual(path.stat().st_size, 1024 * 1024,
                                     'Oversized cache metadata must not be read')
            return original_open(path, *args, **kwargs)

        capture = self.capture()
        capture.opener.open = Mock(return_value=Response(FILE, self.pdf))
        with patch.object(Path, 'open', bounded_open):
            self.assertEqual(capture.fetch(FILE, 100_000)[1], self.pdf)
        self.assertEqual(capture.stats['cache_repairs'], 1)
        self.assertEqual(capture.stats['assets_downloaded'], 1)

    def test_force_refresh_bypasses_cache_and_loader_and_updates_future_runs(self):
        self.prime()
        updated = b'%PDF-1.7\nreplacement'
        capture = self.capture(refresh_assets=True,
                               asset_loader=Mock(side_effect=AssertionError('Refresh used DB')))
        capture.opener.open = Mock(return_value=Response(FILE, updated))
        self.assertEqual(capture.fetch(FILE, 100_000)[1], updated)
        self.assertEqual(capture.stats['assets_downloaded'], 1)
        later = self.capture()
        later.opener.open = Mock(side_effect=AssertionError('Refreshed asset requested'))
        self.assertEqual(later.fetch(FILE, 100_000)[1], updated)

    def test_conditional_page_and_sitemap_304_preserve_bytes_and_recheck_http(self):
        for url, body in [(PAGE, HTML), (ORIGIN + '/vismo/sitemap.asp', SITEMAP)]:
            with self.subTest(url=url):
                original = self.prime(url, body, {'ETag': '"v1"', 'Last-Modified': 'Mon, 05 Oct 2026 08:00:00 GMT'})
                capture = self.capture()

                def unchanged(request, timeout):
                    self.assertEqual(request.get_header('If-none-match'), '"v1"')
                    self.assertEqual(request.get_header('If-modified-since'), 'Mon, 05 Oct 2026 08:00:00 GMT')
                    raise HTTPError(url, 304, 'Not Modified', Message(), None)

                capture.opener.open = Mock(side_effect=unchanged)
                result, raw = capture.fetch(url, 100_000)
                self.assertEqual(raw, body)
                self.assertEqual(result['sha256'], original[0]['sha256'])
                self.assertEqual(capture.stats['pages_not_modified'], 1)
                self.assertEqual(capture.stats['network_requests'], 1)
                self.assertEqual(capture.stats['downloaded_bytes'], 0)
                self.assertEqual(capture.blob(result['sha256']), body)

    def test_page_http_failure_never_uses_stale_cached_body(self):
        self.prime(PAGE, HTML, {'ETag': '"v1"'})
        for status in (404, 503):
            with self.subTest(status=status):
                capture = self.capture()
                capture.opener.open = Mock(side_effect=HTTPError(PAGE, status, 'Unavailable', Message(), None))
                with patch('legacy.time.sleep'), self.assertRaises((HTTPError, ValueError)):
                    capture.fetch(PAGE, 100_000)
                self.assertEqual(capture.stats['pages_not_modified'], 0)
                self.assertFalse((capture.root / 'responses' / (digest(PAGE.encode()) + '.json')).exists())

    def test_changed_page_replaces_conditional_cache_and_is_used_by_later_304(self):
        self.prime(PAGE, HTML, {'ETag': '"v1"'})
        modified = HTML.replace(b'Preserved content.', b'Updated content.')
        second = self.capture()
        second.opener.open = Mock(return_value=Response(PAGE, modified, {'ETag': '"v2"'}))
        self.assertEqual(second.fetch(PAGE, 100_000)[1], modified)
        third = self.capture()
        third.opener.open = Mock(side_effect=HTTPError(PAGE, 304, 'Not Modified', Message(), None))
        self.assertEqual(third.fetch(PAGE, 100_000)[1], modified)
        self.assertEqual(third.opener.open.call_args[0][0].get_header('If-none-match'), '"v2"')

    def test_page_without_validators_is_downloaded_again(self):
        self.prime(PAGE, HTML)
        capture = self.capture()
        capture.opener.open = Mock(return_value=Response(PAGE, HTML))
        capture.fetch(PAGE, 100_000)
        request = capture.opener.open.call_args[0][0]
        self.assertIsNone(request.get_header('If-none-match'))
        self.assertIsNone(request.get_header('If-modified-since'))
        self.assertEqual(capture.stats['pages_downloaded'], 1)

    def test_unrequested_304_without_valid_cache_is_an_error(self):
        capture = self.capture()
        capture.opener.open = Mock(side_effect=HTTPError(PAGE, 304, 'Not Modified', Message(), None))
        with self.assertRaises(HTTPError):
            capture.fetch(PAGE, 100_000)

    def test_default_local_bundle_is_still_resumable_without_network(self):
        root = self.root / 'offline'
        first = Capture(root, delay=0, verbose=False)
        first.opener.open = Mock(return_value=Response(FILE, self.pdf))
        original = first.fetch(FILE, 100_000)
        second = Capture(root, delay=0, verbose=False)
        second.opener.open = Mock(side_effect=AssertionError('Resume requested network'))
        self.assertEqual(second.fetch(FILE, 100_000), original)

    def test_pruning_removes_old_page_versions_but_keeps_seen_assets_and_run_bundle(self):
        asset, _ = self.prime()
        old = self.capture()
        old.opener.open = Mock(return_value=Response(PAGE, HTML))
        original, _ = old.fetch(PAGE, 100_000)
        changed = HTML.replace(b'Preserved content.', b'New content.')
        current = self.capture()
        current.opener.open = Mock(return_value=Response(PAGE, changed))
        updated, _ = current.fetch(PAGE, 100_000)
        current.prune_cache()
        self.assertFalse((self.cache / 'objects' / original['sha256']).exists())
        self.assertTrue((self.cache / 'objects' / updated['sha256']).exists())
        self.assertTrue((self.cache / 'objects' / asset['sha256']).exists())
        self.assertEqual(old.blob(original['sha256']), HTML)
        self.assertEqual(current.stats['cache_objects_pruned'], 1)

    def test_pruning_aborts_if_an_index_cannot_be_read(self):
        self.prime()
        orphan = self.cache / 'objects' / digest(b'orphan')
        orphan.write_bytes(b'orphan')
        (self.cache / 'responses' / 'broken.json').write_text('incomplete')
        current = self.capture()
        current.prune_cache()
        self.assertTrue(orphan.exists())
        self.assertEqual(current.stats['cache_objects_pruned'], 0)


if __name__ == '__main__':
    unittest.main()
