"""Offline extraction and PostgreSQL migration regressions. No requests to the real website."""
import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from datetime import date

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import Capture, canonical, digest, extract, source_key, event_timing, markdown, dates
from bs4 import BeautifulSoup
from legacy_import import fingerprint, import_bundle, load_bundle, prepare
from legacy_reconcile import reconcile, require_local_preview
from legacy_reimport import reset_import, reset_library
from legacy_scope import notice_values
from postgres import connect, temporary_database

URL = 'https://vysker.cz/dokumenty/ms-1053'
FILE = 'https://vysker.cz/assets/File.ashx?id_dokumenty=123&id_org=18774'
HTML = f'''<title>Dokumenty: obec Vyskeř</title><div id="menu"><a href="/obec/ds-50/p1=42">Obec</a></div>
<div id="stred"><div id="zahlavi"><h2>Dokumenty</h2></div><div class="editor">
<h3>Jednání</h3><p>Český <strong>text</strong>.</p><ul><li><a href="{FILE}">Zápis</a> Vyvěšeno: 2. 3. 2020</li></ul>
<script>alert('bad')</script><img src="https://tracker.test/pixel" alt="external">
<div id="kalakci">Calendar widget <a href="/ap?datum=2026">Next</a></div>
<form>Search noise</form></div><div class="dpopis">Vytvořeno / změněno: 1.3.2020 / 2.3.2020</div></div>'''


def bundle(root, notice=False, document_links=True):
    capture = Capture(root, 0)
    objects = Path(root) / 'objects'
    objects.mkdir()
    raw_html = HTML if document_links else '<div id="stred"><h1>Historie</h1><p>Historie obce.</p></div>'
    page = extract(raw_html.encode(), URL)
    if notice:
        page.update(kind='notice', dates={'published_on': '2020-03-02', 'withdraw_on': '2020-03-20'})
    file = dict(url=FILE, title='Zápis', parents=[page['key']] if document_links else [], dates={'published_on': '2020-03-02'},
                name='zapis.pdf', mime='application/pdf', evidence='Vyvěšeno: 2. 3. 2020')
    for item, raw in [(page, raw_html.encode()), (file, b'%PDF-1.7\nfixture\n%%EOF')]:
        sha = digest(raw)
        (objects / sha).write_bytes(raw)
        item['capture'] = {'sha256': sha, 'size': len(raw), 'captured_at': '2026-09-30T12:00:00+00:00'}
    manifest = dict(version=1, origin='https://vysker.cz', pages=[page], assets=[file], aliases={}, errors=[], complete=True)
    (Path(root) / 'manifest.json').write_text(json.dumps(manifest))
    return manifest


def add_page(root, data, html, url='https://vysker.cz/historie/ms-7777'):
    raw = html.encode()
    page = extract(raw, url)
    sha = digest(raw)
    (Path(root) / 'objects' / sha).write_bytes(raw)
    page['capture'] = dict(sha256=sha, size=len(raw), captured_at='2026-09-30T12:00:00+00:00')
    data['pages'].append(page)
    (Path(root) / 'manifest.json').write_text(json.dumps(data))
    return page


def navigation_bundle(root):
    data = bundle(root)
    board = data['pages'][0]
    raw = ('<p class="cesta"><a>Úřad a samospráva</a><a>Úřední deska</a><span>Dotace</span></p>' + HTML).encode()
    sha = digest(raw)
    (Path(root) / 'objects' / sha).write_bytes(raw)
    board['capture'] = dict(board['capture'], sha256=sha, size=len(raw))
    normal = copy.deepcopy(board)
    normal.update(key='ms:9999', url='https://vysker.cz/ostatni/ms-9999', title='Obyčejná stránka')
    # The menu mentions the board, but the breadcrumb does not place this page there.
    normal_raw = ('<p class="cesta"><a>Obec</a><span>Historie</span></p><nav>Úřední deska</nav>' + HTML).encode()
    sha = digest(normal_raw)
    (Path(root) / 'objects' / sha).write_bytes(normal_raw)
    normal['capture'] = dict(normal['capture'], sha256=sha, size=len(normal_raw))
    data['pages'].append(normal)
    data['assets'][0].update(evidence='', dates={}, parents=[normal['key'], board['key']])
    ordinary = copy.deepcopy(data['assets'][0])
    ordinary.update(url=FILE.replace('123', '456'), title='Obyčejný dokument', parents=[normal['key']])
    data['assets'].append(ordinary)
    (Path(root) / 'manifest.json').write_text(json.dumps(data))
    return data


class ExtractionTests(unittest.TestCase):
    def test_preview_requires_both_dates_for_current_notices(self):
        cases = [
            ({}, 'archived', ['published_on', 'withdraw_on']),
            ({'published_on':'2026-09-01'}, 'archived', ['withdraw_on']),
            ({'withdraw_on':'2026-11-01'}, 'archived', ['published_on']),
            ({'published_on':'2026-11-01'}, 'archived', ['withdraw_on']),
            ({'published_on':'2026-09-01', 'deadline_on':'2026-11-01'}, 'archived', ['withdraw_on']),
            ({'published_on':'2026-09-01', 'withdraw_on':'2026-11-01'}, 'published', []),
            ({'published_on':'2026-10-01', 'withdraw_on':'2026-11-01'}, 'published', []),
            ({'published_on':'2026-09-01', 'withdraw_on':'2026-10-01'}, 'archived', []),
            ({'published_on':'2026-11-01', 'withdraw_on':'2026-12-01'}, 'draft', []),
            ({'published_on':'2026-11-01', 'withdraw_on':'2026-10-01'}, 'archived', ['withdraw_on']),
        ]
        for extracted, status, missing in cases:
            with self.subTest(dates=extracted):
                item = dict(title='Zápis', url=FILE, dates=extracted)
                values = notice_values(item, {'Ostatní':1}, {}, {}, preview=True, as_of=date(2026,10,1))
                self.assertEqual(values['status'], status)
                self.assertEqual(values['metadata']['preview_archive_missing_dates'], missing)
                self.assertEqual(notice_values(item, {'Ostatní':1}, {}, {})['status'], 'draft')

    def test_posting_dates_accept_missing_colon_but_never_borrow_a_year(self):
        self.assertEqual(dates('Vyvěšeno 7. 9. 2022, Lhůta do: 30. 9. 2022'),
                         {'published_on':'2022-09-07','deadline_on':'2022-09-30'})
        self.assertEqual(dates('Vyvěšeno: 20. 12., Lhůta do: 5. 1. 2020'),
                         {'deadline_on':'2020-01-05'})
        self.assertEqual(dates('Vyvěšno: 22. 6. 2026'), {'published_on':'2026-06-22'})
        self.assertEqual(dates('Vyvěšeno: 31. 2. 2026'), {})

    def test_notice_preview_refuses_production_or_remote_targets(self):
        values=dict(OBEC_PRODUCTION='false',OBEC_VEREJNA_URL='http://127.0.0.1:3000',
                    OBEC_DATABAZE='postgresql://user@127.0.0.1/test')
        with patch.dict(os.environ,values):
            require_local_preview()
            for key,value in [('OBEC_PRODUCTION','true'),('OBEC_VEREJNA_URL','https://vysker.cz'),
                              ('OBEC_DATABAZE','postgresql://user@db.example.test/test')]:
                with patch.dict(os.environ,{key:value}), self.assertRaisesRegex(ValueError,'local'):
                    require_local_preview()

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

    def test_navigation_classification_is_exclusive_even_without_dates(self):
        data = navigation_bundle(self.root)
        with patch.dict(os.environ, {'OBEC_PRODUCTION':'false', 'OBEC_VEREJNA_URL':'http://127.0.0.1:3000', 'OBEC_DATABAZE':self.url}):
            with connect(self.url) as conn:
                result = import_bundle(conn, self.root, publish_content=True, classify_navigation=True, preview_notices=True)
                self.assertEqual(result['classification'], {'notice':1, 'document':1})
                self.assertEqual(conn.execute('SELECT title,published_on,withdraw_on,status FROM notices').fetchone(), ('Zápis',None,None,'archived'))
                self.assertEqual(conn.execute('SELECT title FROM documents').fetchone(), ('Obyčejný dokument',))
                self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 2)
                self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
                self.assertEqual(len(result['skipped_document_pages']), 2)
                self.assertEqual(conn.execute('SELECT count(*) FROM notice_events').fetchone()[0], 0)
                self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)
                notice_metadata = conn.execute('SELECT metadata FROM legacy_notice_imports').fetchone()[0]
                self.assertIn(data['pages'][0]['key'], notice_metadata['navigation'])
                again = import_bundle(conn, self.root, publish_content=True, classify_navigation=True, preview_notices=True)
                self.assertEqual(again['new'], [])
                self.assertEqual(len(again['unchanged']), 2)

    def test_local_reset_preserves_pages_and_file_links_and_restores_protection(self):
        navigation_bundle(self.root)
        with patch.dict(os.environ, {'OBEC_PRODUCTION':'false', 'OBEC_VEREJNA_URL':'http://127.0.0.1:3000', 'OBEC_DATABAZE':self.url}):
            with connect(self.url) as conn:
                import_bundle(conn, self.root, publish_content=True)
                attachment_id = conn.execute('SELECT id FROM attachments ORDER BY id LIMIT 1').fetchone()[0]
                conn.execute("INSERT INTO pages(slug,title,content,updated_at) VALUES ('local','Local page',%s,'2026-10-01T12:00:00+00:00')",
                             (f'Local edit [Zápis](</api/v1/attachments/{attachment_id}>)',))
                pages = conn.execute('SELECT * FROM pages ORDER BY id').fetchall()
                ids = dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL'))
                with self.assertRaisesRegex(ValueError, 'reconciliation'):
                    import_bundle(conn, self.root, classify_navigation=True)
                counts, preserved = reset_library(conn)
                self.assertEqual(counts['documents'], 2)
                self.assertEqual(ids, preserved)
                import_bundle(conn, self.root, publish_content=True, classify_navigation=True, preview_notices=True, attachment_ids=preserved)
                self.assertEqual(pages, conn.execute('SELECT * FROM pages ORDER BY id').fetchall())
                self.assertEqual(ids, dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL')))
                self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 1)
                self.assertEqual(conn.execute('SELECT count(*) FROM notices').fetchone()[0], 1)
                self.assertEqual(conn.execute("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled='D'").fetchone()[0], 0)
            with self.assertRaisesRegex(RuntimeError, 'abort'), connect(self.url) as conn:
                reset_library(conn)
                raise RuntimeError('abort')
            with connect(self.url) as conn:
                self.assertEqual(conn.execute('SELECT count(*) FROM notices').fetchone()[0], 1)
                self.assertEqual(conn.execute("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled='D'").fetchone()[0], 0)

    def test_full_reimport_removes_only_imported_records_and_rolls_back_failures(self):
        data = navigation_bundle(self.root)
        page = add_page(self.root, data, '<div id="stred"><h1>Historie</h1><p>Text.</p></div>')
        event = add_page(self.root, data, '<div id="stred"><h1>Setkání</h1><p>Na návsi.</p></div>',
                         'https://vysker.cz/setkani/a-1')
        event['event'] = dict(event_timing('1.10.2026 19:00'), location='Náves')
        (Path(self.root) / 'manifest.json').write_text(json.dumps(data))
        tables = ('pages', 'page_revisions', 'page_images', 'events', 'documents', 'notices',
                  'attachments', 'notice_events', 'legacy_sources', 'legacy_notice_imports', 'legacy_sync_reviews')
        with patch.dict(os.environ, {'OBEC_PRODUCTION':'false', 'OBEC_VEREJNA_URL':'http://127.0.0.1:3000', 'OBEC_DATABAZE':self.url}):
            with connect(self.url) as conn:
                import_bundle(conn, self.root, classify_navigation=True)
                local_page = conn.execute("INSERT INTO pages(slug,title,content,updated_at) VALUES ('local','Local','Edited','2026-10-01') RETURNING id").fetchone()[0]
                local_document = conn.execute("INSERT INTO documents(title,created_at) VALUES ('Local','2026-10-01') RETURNING id").fetchone()[0]
                conn.execute("INSERT INTO notices(title) VALUES ('Local')")
                conn.execute("INSERT INTO events(title,starts_at,ends_at,updated_at) VALUES ('Local','2026-10-01','2026-10-02','2026-10-01')")
                conn.execute("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,'local.pdf','application/pdf',1,%s)", (local_document, b'x'))
                conn.execute('''INSERT INTO page_revisions(page_id,version,title,content,slug,published,saved_at)
                    SELECT id,version,title,content,slug,published,updated_at FROM pages WHERE id=%s''', (local_page,))
                conn.execute('''INSERT INTO page_images(page_id,name,content_type,size_bytes,width,height,data,sha256,created_at)
                    SELECT id,'local.png','image/png',1,1,1,%s,'test','2026-10-01' FROM pages''', (b'x',))
                conn.execute("INSERT INTO notice_events(notice_id,occurred_at,kind,payload,sha256) SELECT id,'2026-10-01','test','{}','test' FROM notices")
                attachment_ids = dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL'))
                # The updated source page now links to a document and must disappear on reimport.
                page['assets'].append(dict(url=FILE))
                page['content'] += f'\n[Zápis](<{FILE}>)'
                (Path(self.root) / 'manifest.json').write_text(json.dumps(data))
                import_bundle(conn, self.root, classify_navigation=True, sync=True)
                self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sync_reviews').fetchone()[0], 1)
                before = {table: conn.execute('SELECT * FROM '+table+' ORDER BY 1').fetchall() for table in tables}
                local = {table: conn.execute('SELECT * FROM '+table+" WHERE title='Local' ORDER BY id").fetchall()
                         for table in ('pages', 'events', 'documents', 'notices')}
            with self.assertRaisesRegex(ValueError, 'Invalid mapped page slug'), connect(self.url) as conn:
                reset_import(conn)
                import_bundle(conn, self.root, page_map={'ms:1': 'invalid/slug'})
            with connect(self.url) as conn:
                self.assertEqual(before, {table: conn.execute('SELECT * FROM '+table+' ORDER BY 1').fetchall() for table in tables})
                self.assertEqual(conn.execute("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled='D'").fetchone()[0], 0)
                counts, preserved = reset_import(conn)
                self.assertEqual(counts['pages'], 1)
                self.assertEqual(counts['events'], 1)
                self.assertEqual(counts['documents'], 1)
                self.assertEqual(counts['notices'], 1)
                self.assertEqual(counts['attachments'], 2)
                self.assertEqual(counts['page_images'], 1)
                self.assertEqual(counts['page_revisions'], 1)
                self.assertEqual(counts['notice_events'], 1)
                self.assertEqual(counts['legacy_sync_reviews'], 1)
                self.assertEqual(preserved, attachment_ids)
                result = import_bundle(conn, self.root, classify_navigation=True, attachment_ids=preserved)
                self.assertEqual(len(result['skipped_document_pages']), 3)
                self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 1)
                self.assertEqual(conn.execute('SELECT count(*) FROM page_images').fetchone()[0], 1)
                self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 1)
                self.assertEqual(conn.execute('SELECT count(*) FROM events').fetchone()[0], 2)
                self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 3)
                self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sync_reviews').fetchone()[0], 0)
                self.assertEqual(local, {table: conn.execute('SELECT * FROM '+table+" WHERE title='Local' ORDER BY id").fetchall() for table in local})
                self.assertEqual(preserved, dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL')))
                self.assertEqual(conn.execute("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled='D'").fetchone()[0], 0)
            with patch.dict(os.environ, {'OBEC_PRODUCTION': 'true'}), connect(self.url) as conn:
                with self.assertRaisesRegex(ValueError, 'local'):
                    reset_import(conn)

    def test_navigation_notice_deadline_is_not_a_withdrawal_date(self):
        item = dict(title='Zápis', url=FILE, parents=['ms:1'], evidence='Vyvěšeno: 2. 3. 2020, Lhůta do: 20. 3. 2020')
        values = notice_values(item, {'Ostatní':1}, {'ms:1':['Úřední deska']}, {}, preview=True, as_of=date(2026,10,1))
        self.assertEqual(values['published_on'], date(2020,3,2))
        self.assertIsNone(values['withdraw_on'])
        self.assertEqual(values['status'], 'archived')
        self.assertEqual(values['metadata']['preview_archive_missing_dates'], ['withdraw_on'])
        item['evidence'] += ' Sejmuto: 21. 3. 2020'
        values = notice_values(item, {'Ostatní':1}, {}, {})
        self.assertEqual(values['withdraw_on'], date(2020,3,21))
        self.assertEqual(values['status'], 'draft')
        item['evidence'] = 'Vyvěšeno: 20. 12., Lhůta do: 5. 1. 2020'
        item['dates'] = {'published_on':'2020-12-20'}
        values = notice_values(item, {'Ostatní':1}, {}, {})
        self.assertIsNone(values['published_on'])

    def test_draft_import_is_idempotent_preserves_dates_and_creates_no_mail(self):
        bundle(self.root)
        with connect(self.url) as conn:
            conn.execute('SET TRANSACTION READ ONLY')
            report, _, _ = prepare(conn, self.root)
            self.assertEqual(len(report['new']), 1)
            self.assertEqual(report['skipped_document_pages'], [{
                'key': source_key(URL), 'url': URL, 'documents': [source_key(FILE)], 'already_imported': False}])
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
        with connect(self.url) as conn:
            first = import_bundle(conn, self.root)
            self.assertEqual(len(first['new']), 1)
        with connect(self.url) as conn:
            second = import_bundle(conn, self.root)
            self.assertEqual(len(second['new']), 0)
            self.assertEqual(len(second['unchanged']), 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT published_at FROM documents').fetchone()[0], None)
            self.assertEqual(conn.execute('SELECT source_key,destination FROM legacy_sources').fetchall(),
                             [(source_key(FILE), '/api/v1/attachments/1')])
            self.assertEqual(conn.execute("SELECT metadata->'dates'->>'published_on' FROM legacy_sources WHERE attachment_id IS NOT NULL").fetchone()[0], '2020-03-02')

    def test_conflicts_preserve_editor_changes_and_abort_whole_batch(self):
        data = bundle(self.root, document_links=False)
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

    def test_all_documents_import_without_their_container_or_duplicate_files(self):
        data = bundle(self.root)
        second = copy.deepcopy(data['assets'][0])
        second.update(url=FILE.replace('123', '456'), title='Další zápis')
        data['assets'].append(second)
        data['pages'][0]['assets'].extend([dict(url=second['url']), dict(url=FILE)])
        data['pages'][0]['content'] += f'\n[Další zápis](<{second["url"]}>)'
        add_page(self.root, data, f'<div id="stred"><h1>Další stránka</h1><a href="{FILE}">Zápis</a></div>')
        with connect(self.url) as conn:
            first = import_bundle(conn, self.root, publish_content=True, page_map={source_key(URL): 'dokumenty'})
            self.assertEqual(len(first['skipped_document_pages']), 2)
            self.assertEqual(first['skipped_document_pages'][0]['documents'], sorted([source_key(FILE), source_key(second['url'])]))
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents WHERE status=\'published\'').fetchone()[0], 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 2)
            repeated = import_bundle(conn, self.root, publish_content=True)
            self.assertEqual(repeated['new'], [])
            self.assertEqual(len(repeated['unchanged']), 2)

    def test_plain_pages_and_image_articles_keep_their_content_and_revisions(self):
        data = bundle(self.root, document_links=False)
        image = copy.deepcopy(data['assets'][0])
        # The importer verifies file signatures, image decoding belongs to the upload API.
        raw = b'\x89PNG\r\n\x1a\nfixture'
        sha = digest(raw)
        (Path(self.root) / 'objects' / sha).write_bytes(raw)
        image.update(url='https://vysker.cz/assets/Image.ashx?id_obrazky=10', name='kaple.png', mime='image/png',
                     parents=['ms:7777'], capture=dict(image['capture'], sha256=sha, size=len(raw)))
        data['assets'].append(image)
        add_page(self.root, data, f'<div id="stred"><h1>Kaple</h1><p>Naše kaple.</p><img src="{image["url"]}" alt="Kaple"></div>')
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root)
            self.assertEqual(report['skipped_document_pages'], [])
            self.assertEqual(conn.execute('SELECT count(*) FROM pages WHERE NOT published').fetchone()[0], 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 2)
            self.assertIn('![Kaple](</api/v1/legacy-media/', conn.execute("SELECT content FROM pages WHERE title='Kaple'").fetchone()[0])
            self.assertEqual(import_bundle(conn, self.root)['new'], [])

    def test_existing_document_pages_keep_local_edits_and_source_conflicts(self):
        data = bundle(self.root)
        page = data['pages'][0]
        with connect(self.url) as conn:
            # Reproduce a page imported before document containers were excluded.
            page_id = conn.execute("INSERT INTO pages(slug,title,content,updated_at) VALUES ('existing','Local title','Local edit','2026-10-01T12:00:00+00:00') RETURNING id").fetchone()[0]
            conn.execute('''INSERT INTO legacy_sources
                (source_key,source_url,fingerprint,captured_at,imported_at,metadata,page_id,destination)
                VALUES (%s,%s,%s,%s,%s,'{}',%s,'/stranky/existing')''',
                (page['key'], page['url'], fingerprint(page), page['capture']['captured_at'], page['capture']['captured_at'], page_id))
            report = import_bundle(conn, self.root, sync=True)
            self.assertTrue(report['skipped_document_pages'][0]['already_imported'])
            self.assertEqual(report['unchanged'], [page['key']])
            self.assertEqual(conn.execute('SELECT title,content FROM pages').fetchall(), [('Local title', 'Local edit')])
            page['content'] = 'Changed source'
            (Path(self.root) / 'manifest.json').write_text(json.dumps(data))
            report = import_bundle(conn, self.root, sync=True)
            self.assertEqual(report['conflicts'], [page['key']])
            self.assertEqual(report['pending_reviews'], 1)
            self.assertEqual(conn.execute('SELECT content FROM pages').fetchall(), [('Local edit',)])

    def test_links_to_skipped_pages_remain_available_for_editorial_review(self):
        data = bundle(self.root)
        add_page(self.root, data, f'<div id="stred"><h1>Přehled</h1><a href="{URL}">Dokumenty</a></div>')
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, publish_content=True)
            self.assertEqual(report['unresolved_internal_links'], [URL])
            self.assertIn(URL, conn.execute('SELECT content FROM pages').fetchone()[0])
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sources WHERE source_key=%s', (source_key(URL),)).fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT destination FROM legacy_sources WHERE source_key=%s', (source_key(FILE),)).fetchone()[0], '/api/v1/attachments/1')

    def test_failed_document_capture_does_not_silently_drop_its_page(self):
        data = bundle(self.root)
        data['assets'][0]['error'] = '404'
        data.update(complete=False, errors=[{'url': FILE, 'error': '404'}])
        (Path(self.root) / 'manifest.json').write_text(json.dumps(data))
        with connect(self.url) as conn:
            report = import_bundle(conn, self.root, allow_incomplete=True)
            self.assertEqual(report['skipped_document_pages'], [])
            self.assertEqual(report['unresolved_internal_links'], [FILE])
            self.assertEqual(len(report['capture_errors']), 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 0)

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

    def test_file_notices_reconcile_idempotently_without_changing_documents(self):
        data=bundle(self.root)
        file=data['assets'][0]
        file['dates']={}
        file['evidence']='Vyvěšeno 2. 3. 2020, Lhůta do: 20. 3. 2020'
        (Path(self.root)/'manifest.json').write_text(json.dumps(data))
        mapping={data['pages'][0]['key']:'Zastupitelstvo'}
        with connect(self.url) as conn:
            import_bundle(conn,self.root,publish_content=True)
        with connect(self.url) as conn:
            conn.execute('SET TRANSACTION READ ONLY')
            plan=reconcile(conn,self.root,mapping)
            self.assertEqual(len(plan['notices_created']),1)
            self.assertEqual(conn.execute('SELECT count(*) FROM notices').fetchone()[0],0)
        with connect(self.url) as conn:
            result=reconcile(conn,self.root,mapping,apply=True)
            self.assertEqual(len(result['dates_updated']),0)
            self.assertEqual(conn.execute('SELECT source_published_on,published_at,status FROM documents').fetchone(),
                             (date(2020,3,2),None,'published'))
            self.assertEqual(conn.execute('SELECT status,withdraw_on,published_at FROM notices').fetchone(),('draft',None,None))
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0],2)
            self.assertEqual(conn.execute('SELECT count(DISTINCT sha256) FROM attachments').fetchone()[0],1)
            self.assertEqual(conn.execute('SELECT count(*) FROM notice_events').fetchone()[0],0)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0],0)
        with connect(self.url) as conn:
            conn.execute("UPDATE notices SET title='Local edit'")
            repeated=reconcile(conn,self.root,mapping,apply=True)
            self.assertEqual(len(repeated['notices_created']),0)
            self.assertEqual(len(repeated['notices_unchanged']),1)
            self.assertEqual(conn.execute('SELECT title FROM notices').fetchone()[0],'Local edit')

    def test_local_notice_preview_keeps_source_dates_separate_from_publication_evidence(self):
        data=bundle(self.root)
        data['assets'][0]['evidence']='Vyvěšeno: 2. 3. 2020, Lhůta do: 20. 3. 2020'
        (Path(self.root)/'manifest.json').write_text(json.dumps(data))
        with connect(self.url) as conn:
            import_bundle(conn,self.root,publish_content=True)
        with patch.dict(os.environ,{'OBEC_PRODUCTION':'false','OBEC_VEREJNA_URL':'http://127.0.0.1:3000','OBEC_DATABAZE':self.url}):
            with connect(self.url) as conn:
                reconcile(conn,self.root,{data['pages'][0]['key']:'Zastupitelstvo'},apply=True,preview=True,as_of=date(2026,10,1))
                self.assertEqual(conn.execute('SELECT status,published_on,withdraw_on,published_at,withdrawn_at FROM notices').fetchone(),
                                 ('archived',date(2020,3,2),date(2020,3,21),None,None))
                self.assertTrue(conn.execute('SELECT preview_published FROM legacy_notice_imports').fetchone()[0])
                self.assertEqual(conn.execute('SELECT count(*) FROM notice_events').fetchone()[0],0)
                self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0],0)

    def test_reconciliation_does_not_publish_edited_documents(self):
        data=bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn,self.root,publish_content=True)
            conn.execute("UPDATE documents SET title='Local title'")
        with connect(self.url) as conn:
            result=reconcile(conn,self.root,{data['pages'][0]['key']:'Zastupitelstvo'},apply=True)
            self.assertEqual(len(result['notices_created']),0)
            self.assertEqual(conn.execute('SELECT title FROM documents').fetchone()[0],'Local title')

    def test_reconciliation_keeps_incomplete_posting_dates_unknown(self):
        data=bundle(self.root)
        data['assets'][0].update(dates={},evidence='Vyvěšeno: 20. 12., Lhůta do: 5. 1. 2020')
        (Path(self.root)/'manifest.json').write_text(json.dumps(data))
        with connect(self.url) as conn:
            import_bundle(conn,self.root,publish_content=True)
            result=reconcile(conn,self.root,{data['pages'][0]['key']:'Zastupitelstvo'},apply=True)
            self.assertEqual(result['notices_created'],[])
            self.assertIsNone(conn.execute('SELECT source_published_on FROM documents').fetchone()[0])
            self.assertEqual(len(result['review']),1)


if __name__ == '__main__':
    unittest.main()
