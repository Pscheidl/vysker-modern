"""Offline crawler completeness regressions, without PostgreSQL or live requests."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import Capture, ORIGIN


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


if __name__ == '__main__':
    unittest.main()
