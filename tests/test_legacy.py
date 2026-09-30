"""Offline extraction and PostgreSQL migration regressions. No requests to the real website."""
import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import Capture, canonical, digest, extract, source_key, event_timing, markdown
from bs4 import BeautifulSoup
from legacy_import import import_bundle, load_bundle, prepare
from postgres import connect, temporary_database

URL = 'https://vysker.cz/dokumenty/ms-1053'
FILE = 'https://vysker.cz/assets/File.ashx?id_dokumenty=123&id_org=18774'
HTML = f'''<title>Dokumenty: obec Vyskeř</title><div id="menu"><a href="/obec/ds-50/p1=42">Obec</a></div>
<div id="stred"><div id="zahlavi"><h2>Dokumenty</h2></div><div class="editor">
<h3>Jednání</h3><p>Český <strong>text</strong>.</p><ul><li><a href="{FILE}">Zápis</a> Vyvěšeno: 2. 3. 2020</li></ul>
<script>alert('bad')</script><img src="https://tracker.test/pixel" alt="external">
<div id="kalakci">Calendar widget <a href="/ap?datum=2026">Next</a></div>
<form>Search noise</form></div><div class="dpopis">Vytvořeno / změněno: 1.3.2020 / 2.3.2020</div></div>'''


def bundle(root, notice=False):
    capture = Capture(root, 0)
    objects = Path(root) / 'objects'
    objects.mkdir()
    page = extract(HTML.encode(), URL)
    if notice:
        page.update(kind='notice', dates={'published_on': '2020-03-02', 'withdraw_on': '2020-03-20'})
    file = dict(url=FILE, title='Zápis', parents=[page['key']], dates={'published_on': '2020-03-02'},
                name='zapis.pdf', mime='application/pdf', evidence='Vyvěšeno: 2. 3. 2020')
    for item, raw in [(page, HTML.encode()), (file, b'%PDF-1.7\nfixture\n%%EOF')]:
        sha = digest(raw)
        (objects / sha).write_bytes(raw)
        item['capture'] = {'sha256': sha, 'size': len(raw), 'captured_at': '2026-09-30T12:00:00+00:00'}
    manifest = dict(version=1, origin='https://vysker.cz', pages=[page], assets=[file], aliases={}, errors=[], complete=True)
    (Path(root) / 'manifest.json').write_text(json.dumps(manifest))
    return manifest


class ExtractionTests(unittest.TestCase):
    def test_event_dates_do_not_invent_known_times(self):
        event = event_timing('29.9.2026 19:00 - 29.9.2026')
        self.assertTrue(event['start_time_known'])
        self.assertFalse(event['end_time_known'])
        self.assertEqual(event['starts_at'], '2026-09-29T19:00:00+02:00')
        self.assertIsNotNone(event_timing('9.3.2024 9:00 - 9.3.2024'))
        self.assertIsNone(event_timing('25.10.2026 2:30 - 25.10.2026 4:00'))
        self.assertIsNone(event_timing('29.3.2026 2:30 - 29.3.2026 4:00'))
        self.assertFalse(event_timing('27.9.2025 - 27.9.2025')['start_time_known'])
        self.assertFalse(event_timing('1.3.2025 13:00')['end_date_known'])
        self.assertFalse(event_timing('11.7.2018')['end_date_known'])
    def test_same_identity_and_no_action_urls(self):
        self.assertEqual(source_key('http://www.vysker.cz/nazev%2Dclanku/d-1020/p1=53'), 'd:1020')
        self.assertEqual(source_key('https://vysker.cz/vismo/dokumenty2.asp?id_org=18774&id=1020&n=old'), 'd:1020')
        self.assertEqual(source_key(FILE), '/assets/File.ashx?id_dokumenty=123&id_org=18774')
        self.assertNotEqual(source_key('/x/ds-12/archiv=1'), source_key('/x/ds-12/archiv=0'))
        for path in ['https://evil.test/a', 'http://vysker.cz@evil.test/', 'http://127.0.0.1/',
                     '/aa/login', '/vismo/dokumenty2.asp?id=1&xvvolba1=2', '/vismo/formulare2.asp?id=1']:
            self.assertIsNone(canonical(path), path)

    def test_content_dates_and_assets_without_template_or_active_content(self):
        page = extract(HTML.encode(), URL)
        self.assertEqual(page['title'], 'Dokumenty')
        self.assertEqual(page['dates']['created_on'], '2020-03-01')
        self.assertEqual(page['assets'][0]['dates']['published_on'], '2020-03-02')
        self.assertIn('**text**', page['content'])
        for forbidden in ['alert', 'Search noise', 'Calendar widget', 'tracker.test', 'Vytvořeno']:
            self.assertNotIn(forbidden, page['content'])
        self.assertIn('https://vysker.cz/obec/ds-50', page['links'])

    def test_hash_and_path_validation(self):
        with tempfile.TemporaryDirectory() as root:
            data = bundle(root)
            load_bundle(root)
            file = Path(root) / 'objects' / data['assets'][0]['capture']['sha256']
            file.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'checksum'):
                load_bundle(root)
            with self.assertRaises(ValueError):
                Capture(root).blob('../../outside')

    def test_linked_images_do_not_produce_nested_markdown_links(self):
        html = '<a href="/assets/Image.ashx?id_org=18774&id_obrazky=10"><img src="/assets/thumb.png" alt="Kaple"></a>'
        value = markdown(BeautifulSoup(html, 'html.parser'), URL)
        self.assertTrue(value.startswith('![Kaple](<https://vysker.cz/assets/Image.ashx'))
        self.assertNotIn('[![', value)


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class ImportTests(unittest.TestCase):
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

    def test_draft_import_is_idempotent_preserves_dates_and_creates_no_mail(self):
        bundle(self.root)
        with connect(self.url) as conn:
            conn.execute('SET TRANSACTION READ ONLY')
            report, _, _ = prepare(conn, self.root)
            self.assertEqual(len(report['new']), 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
        with connect(self.url) as conn:
            first = import_bundle(conn, self.root)
            self.assertEqual(len(first['new']), 2)
        with connect(self.url) as conn:
            second = import_bundle(conn, self.root)
            self.assertEqual(len(second['new']), 0)
            self.assertEqual(len(second['unchanged']), 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT published FROM pages').fetchone()[0], False)
            self.assertEqual(conn.execute('SELECT published_at FROM documents').fetchone()[0], None)
            self.assertIn('/api/v1/attachments/', conn.execute('SELECT content FROM pages').fetchone()[0])
            self.assertEqual(conn.execute("SELECT metadata->'dates'->>'published_on' FROM legacy_sources WHERE attachment_id IS NOT NULL").fetchone()[0], '2020-03-02')

    def test_conflicts_preserve_editor_changes_and_abort_whole_batch(self):
        data = bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root)
            conn.execute("UPDATE pages SET content='Local editorial change'")
        data['pages'][0]['content'] = 'Changed source'
        (Path(self.root) / 'manifest.json').write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'reconciliation'), connect(self.url) as conn:
            import_bundle(conn, self.root)
        with connect(self.url) as conn:
            self.assertEqual(conn.execute('SELECT content FROM pages').fetchone()[0], 'Local editorial change')
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 1)

    def test_preview_does_not_publish_notices_or_their_files(self):
        bundle(self.root, notice=True)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, publish_content=True)
            self.assertEqual(conn.execute('SELECT status,retain_attachments,published_at FROM notices').fetchone(), ('draft', False, None))
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 0)
            self.assertIsNotNone(conn.execute('SELECT notice_id FROM attachments').fetchone()[0])
            self.assertEqual(conn.execute('SELECT count(*) FROM notice_events').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)

    def test_incomplete_capture_is_explicit(self):
        data = bundle(self.root)
        data.update(complete=False, errors=[{'url': '/missing', 'error': '404'}])
        (Path(self.root) / 'manifest.json').write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'incomplete'), connect(self.url) as conn:
            import_bundle(conn, self.root)
        with connect(self.url) as conn:
            result = import_bundle(conn, self.root, allow_incomplete=True)
            self.assertEqual(len(result['capture_errors']), 1)

    def test_calendar_import_retains_unknown_time_and_rewrites_attachments(self):
        data = bundle(self.root)
        data['pages'][0]['event'] = dict(event_timing('29.9.2026 19:00 - 29.9.2026'), location='Náves')
        (Path(self.root) / 'manifest.json').write_text(json.dumps(data))
        with connect(self.url) as conn:
            import_bundle(conn, self.root, publish_content=True)
            self.assertEqual(conn.execute('SELECT start_time_known,end_time_known,published FROM events').fetchone(), (True, False, True))
            self.assertIn('/api/v1/attachments/', conn.execute('SELECT description FROM events').fetchone()[0])
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)


if __name__ == '__main__':
    unittest.main()
