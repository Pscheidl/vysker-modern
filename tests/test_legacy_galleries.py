"""Imported Vismo photo galleries retain one article and all historical source links."""
from html import escape
import json
import os
from pathlib import Path
import tempfile
import unittest

import test_legacy as fixtures
from legacy import digest, source_key
from legacy_import import fingerprint, import_bundle, load_bundle
from postgres import connect


ARTICLE = 'https://vysker.cz/historie-spolku/ms-2001'
GALLERY = 'https://vysker.cz/fotografie-spolku/gs-3001'
CAPTIONS = ['Společná fotografie členů', 'Slavnost před kaplí', 'Výlet do okolí']


def photo_url(index, variant=False):
    tail = 'prehravac=1' if variant else f'id_obrazky={4000 + index}'
    return f'https://vysker.cz/fotografie/g-{5000 + index}/{tail}'


def image_url(index, thumbnail=False):
    return f'https://vysker.cz/assets/Image.ashx?id_obrazky={4000 + index + (100 if thumbnail else 0)}'


def gallery_html(count):
    entries = []
    for index in range(count):
        caption = CAPTIONS[index]
        entries.append(f'''<li><div><a href="{photo_url(index)}">
            <img src="{image_url(index, True)}" alt="{caption}"><span>{caption}</span></a>
            <div class="gpn"><ul><li class="gga"><a href="{GALLERY}">Fotografie spolku</a></li>
            <li class="gna">{caption}</li></ul></div></div></li>''')
    return '<div class="obrgalerie kontnahledy"><ul class="nahledy">' + ''.join(entries) + '</ul></div>'


def detail_html(index, parents=(GALLERY,)):
    links = ''.join(f'<a href="{escape(url)}">zpět na galerii</a>' for url in parents)
    return f'''<div id="stred"><div id="zahlavi"><h2>{CAPTIONS[index]}</h2></div>
        <div id="fgzvet"><div id="zpetfg">{links}</div>
        <img id="zvetsenina" src="{image_url(index)}" alt="{CAPTIONS[index]}"></div></div>'''


def save(root, data):
    (Path(root) / 'manifest.json').write_text(json.dumps(data, ensure_ascii=False))


def gallery_bundle(root, count=2, article=True, variant=True):
    (Path(root) / 'objects').mkdir(exist_ok=True)
    data = dict(version=1, origin='https://vysker.cz', pages=[], assets=[], aliases={}, errors=[], complete=True)
    if article:
        fixtures.add_page(root, data, f'''<div id="stred"><div id="zahlavi"><h2>Historie spolku</h2></div>
            <p>Článek se zachovaným historickým textem.</p>
            <div class="vlozfg"><h3>Fotografie</h3>{gallery_html(count)}</div>
            <p>Závěrečná poznámka autora.</p></div>''', ARTICLE)
    backlink = f'<div class="odkazy navratove"><a href="{ARTICLE}">Zpět: Historie spolku</a></div>' if article else ''
    fixtures.add_page(root, data, f'''<div id="stred"><div id="zahlavi"><h2>Fotografie spolku</h2></div>
        {backlink}{gallery_html(count)}</div>''', GALLERY)
    for index in range(count):
        fixtures.add_page(root, data, detail_html(index), photo_url(index))
    if variant:
        fixtures.add_page(root, data, detail_html(0), photo_url(0, True))
    assets = {}
    for page in data['pages']:
        for reference in page['assets']:
            asset = assets.setdefault(reference['url'], dict(reference, parents=[]))
            asset['parents'].append(page['key'])
    for number, asset in enumerate(assets.values()):
        raw = b'\x89PNG\r\n\x1a\n' + asset['url'].encode()
        sha = digest(raw)
        (Path(root) / 'objects' / sha).write_bytes(raw)
        asset.update(name=f'fotografie-{number}.png', mime='image/png',
                     capture=dict(sha256=sha, size=len(raw), captured_at='2026-09-30T12:00:00+00:00'))
    data['assets'] = list(assets.values())
    save(root, data)
    return data


class GalleryExtractionTests(unittest.TestCase):
    def test_embedded_gallery_combines_full_images_and_preserves_raw_source(self):
        with tempfile.TemporaryDirectory() as root:
            data = gallery_bundle(root)
            _, items, _ = load_bundle(root)
        keyed = {item['key']: item for item in items}
        owner = keyed[source_key(ARTICLE)]
        original = next(item for item in data['pages'] if item['key'] == owner['key'])
        self.assertEqual(owner['content'], original['content'])
        self.assertEqual(owner['assets'], original['assets'])
        self.assertIn('Článek se zachovaným historickým textem.', owner['gallery_content'])
        self.assertIn('Závěrečná poznámka autora.', owner['gallery_content'])
        for index in range(2):
            self.assertEqual(owner['gallery_content'].count(image_url(index)), 1)
            self.assertIn(CAPTIONS[index], owner['gallery_content'])
            self.assertNotIn(image_url(index, True), owner['gallery_content'])
        aliases = [source_key(GALLERY), source_key(photo_url(0)), source_key(photo_url(1)), source_key(photo_url(0, True))]
        self.assertEqual(owner['gallery_members'], sorted(aliases))
        for key in aliases:
            self.assertEqual(keyed[key]['page_alias'], owner['key'])

    def test_standalone_gallery_is_one_owner_without_invented_article(self):
        with tempfile.TemporaryDirectory() as root:
            gallery_bundle(root, article=False)
            _, items, _ = load_bundle(root)
        owner = next(item for item in items if item['key'] == source_key(GALLERY))
        self.assertNotIn('page_alias', owner)
        self.assertEqual(owner['gallery_content'].count('!['), 2)
        for item in items:
            if item['key'].startswith('g:'):
                self.assertEqual(item['page_alias'], owner['key'])

    def test_ordinary_image_article_keeps_content_and_identity(self):
        with tempfile.TemporaryDirectory() as root:
            data = gallery_bundle(root)
            ordinary = fixtures.add_page(root, data, f'''<div id="stred"><h1>Samostatný článek</h1>
                <p>Vlastní text článku.</p><img src="{image_url(0)}" alt="Kaple"></div>''',
                'https://vysker.cz/clanek/ms-9000')
            _, items, _ = load_bundle(root)
        actual = next(item for item in items if item['key'] == ordinary['key'])
        self.assertEqual(actual['content'], ordinary['content'])
        self.assertEqual(fingerprint(actual), fingerprint(ordinary))
        self.assertNotIn('page_alias', actual)
        self.assertNotIn('gallery_content', actual)

    def test_gallery_with_its_own_introduction_preserves_that_content(self):
        with tempfile.TemporaryDirectory() as root:
            data = gallery_bundle(root)
            data['pages'] = [item for item in data['pages'] if item['key'] != source_key(GALLERY)]
            fixtures.add_page(root, data, f'''<div id="stred"><div id="zahlavi"><h2>Fotografie spolku</h2></div>
                <div class="odkazy navratove"><a href="{ARTICLE}">Zpět: Historie spolku</a></div>
                <p>Samostatné svědectví autora fotografií, které článek neobsahuje.</p>
                {gallery_html(2)}</div>''', GALLERY)
            _, items, _ = load_bundle(root)
        keyed = {item['key']: item for item in items}
        gallery = keyed[source_key(GALLERY)]
        self.assertNotIn('page_alias', gallery)
        self.assertIn('Samostatné svědectví autora fotografií, které článek neobsahuje.', gallery['gallery_content'])
        self.assertEqual(gallery['gallery_content'].count('!['), 2)
        for item in items:
            if item['key'].startswith('g:'):
                self.assertEqual(item['page_alias'], gallery['key'])
        self.assertNotIn('page_alias', keyed[source_key(ARTICLE)])

    def test_missing_or_ambiguous_gallery_is_deferred_for_review(self):
        for parents in [(), (GALLERY, 'https://vysker.cz/jina-galerie/gs-3002')]:
            with self.subTest(parents=parents), tempfile.TemporaryDirectory() as root:
                data = gallery_bundle(root, variant=False)
                data['pages'] = [item for item in data['pages'] if item['key'] != source_key(photo_url(0))]
                fixtures.add_page(root, data, detail_html(0, parents), photo_url(0))
                _, items, _ = load_bundle(root)
                detail = next(item for item in items if item['key'] == source_key(photo_url(0)))
                self.assertTrue(detail.get('review_required'))
                self.assertNotIn('page_alias', detail)


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class GalleryImportTests(unittest.TestCase):
    setUp = fixtures.ImportTests.setUp
    tearDown = fixtures.ImportTests.tearDown

    def test_aliases_share_article_and_saved_revision_uses_full_images(self):
        data = gallery_bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, publish_content=True)
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 1)
            page_id, body, published = conn.execute('SELECT id,content,published FROM pages').fetchone()
            self.assertTrue(published)
            sources = conn.execute('SELECT source_key,page_id,destination FROM legacy_sources WHERE page_id IS NOT NULL').fetchall()
            self.assertEqual(len(sources), len(data['pages']))
            self.assertEqual({row[1] for row in sources}, {page_id})
            self.assertEqual(len({row[2] for row in sources}), 1)
            self.assertTrue(sources[0][2].startswith('/stranky/'))
            attachments = dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL'))
            for index in range(2):
                self.assertEqual(body.count(f'/api/v1/legacy-media/{attachments[source_key(image_url(index))]}>'), 1)
                self.assertNotIn(f'/api/v1/legacy-media/{attachments[source_key(image_url(index, True))]}>', body)
            self.assertEqual(conn.execute('SELECT content FROM page_revisions').fetchall(), [(body,)])
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)

    def test_repeat_import_preserves_identity_and_does_not_duplicate_revisions(self):
        data = gallery_bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root)
            sources = conn.execute('SELECT * FROM legacy_sources ORDER BY source_key').fetchall()
            repeated = import_bundle(conn, self.root)
            self.assertEqual(repeated['new'], [])
            self.assertEqual(repeated['conflicts'], [])
            self.assertEqual(len(repeated['unchanged']), len(data['pages']) + len(data['assets']))
            self.assertEqual(conn.execute('SELECT * FROM legacy_sources ORDER BY source_key').fetchall(), sources)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 1)

    def test_sync_new_photo_queues_owner_without_overwriting_local_article(self):
        original = gallery_bundle(self.root)
        with connect(self.url) as conn:
            import_bundle(conn, self.root, publish_content=True)
            conn.execute("UPDATE pages SET content='Místní redakční úprava.'")
        changed = gallery_bundle(self.root, count=3)
        # The third photo comes only from a newly discovered slideshow detail.
        # Neither owner's source HTML, extracted content nor assets have changed.
        owners = {item['key']: item for item in original['pages']
                  if item['key'] in {source_key(ARTICLE), source_key(GALLERY)}}
        changed['pages'] = [owners.get(item['key'], item) for item in changed['pages']]
        save(self.root, changed)
        with connect(self.url) as conn:
            result = import_bundle(conn, self.root, publish_content=True, sync=True)
            self.assertIn(source_key(ARTICLE), result['conflicts'])
            self.assertEqual(conn.execute('SELECT content FROM pages').fetchall(), [('Místní redakční úprava.',)])
            pending = conn.execute('SELECT proposal FROM legacy_sync_reviews WHERE source_key=%s', (source_key(ARTICLE),)).fetchone()
            self.assertIsNotNone(pending)
            self.assertIn(image_url(2), pending[0]['gallery_content'])
            review_count = conn.execute('SELECT count(*) FROM legacy_sync_reviews').fetchone()[0]
            import_bundle(conn, self.root, publish_content=True, sync=True)
            self.assertEqual(conn.execute('SELECT count(*) FROM legacy_sync_reviews').fetchone()[0], review_count)
            self.assertEqual(conn.execute('SELECT count(*) FROM page_revisions').fetchone()[0], 1)

    def test_orphan_photo_does_not_create_a_page_or_false_redirect(self):
        data = gallery_bundle(self.root, article=False, variant=False)
        data['pages'] = [item for item in data['pages'] if item['key'].startswith('g:')]
        save(self.root, data)
        with connect(self.url) as conn:
            result = import_bundle(conn, self.root)
            self.assertEqual(conn.execute('SELECT count(*) FROM pages').fetchone()[0], 0)
            self.assertEqual(conn.execute("SELECT count(*) FROM legacy_sources WHERE source_key LIKE 'g:%'").fetchone()[0], 0)
            self.assertTrue({item['key'] for item in data['pages']} <= {item['key'] for item in result['review']})


if __name__ == '__main__':
    unittest.main()
