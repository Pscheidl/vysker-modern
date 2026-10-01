"""Group Vismo photo detail views into their captured article or gallery."""
from collections import defaultdict
import re

from bs4 import BeautifulSoup

from legacy import canonical, escape, markdown, source_key


def _text(node, base):
    value = re.sub(r'\n[ \t]+', '\n', markdown(node, base))
    return re.sub(r'\n{3,}', '\n\n', value).strip()


def _photo(key):
    match = re.match(r'^(g:\d+)(?:[/?]|$)', key or '')
    return match[1] if match else None


def _ordinary(item):
    return ('mime' not in item and not item.get('review_required')
            and not (item.get('kind') == 'notice' and item.get('dates', {}).get('published_on'))
            and not (item.get('event') and len(item['content']) <= 10_000))


def group_galleries(items, capture):
    """Return copies with derived content and aliases, retaining original provenance.

    Only explicit links inside captured content establish ownership. Missing photo
    data goes to review instead of creating an isolated article for the photo.
    """
    result = [dict(item) for item in items]
    records = {item['key']: item for item in result}
    images = {key for key, item in records.items()
              if item.get('mime', '').startswith('image/')}
    documents = {key for key, item in records.items()
                 if 'mime' in item and not item['mime'].startswith('image/')}
    document_pages = {item['key'] for item in result if _ordinary(item)
                      and any(source_key(asset['url']) in documents
                              for asset in item.get('assets', []))}
    bodies = {}

    def body(item):
        key = item['key']
        if key not in bodies:
            soup = BeautifulSoup(capture.blob(item['capture']['sha256']), 'html.parser')
            bodies[key] = soup.select_one('#stred')
        return bodies[key]

    def link_key(node, item):
        url = canonical(node.get('href', ''), item['url'])
        return source_key(url) if url else None

    def hold(item, reason):
        item['review_required'] = item.get('review_required') or reason
        item.pop('page_alias', None)

    details = defaultdict(list)
    families = defaultdict(list)
    for item in result:
        if not _photo(item['key']) or 'mime' in item:
            continue
        node = body(item)
        if node is None:
            hold(item, 'Photo detail has no captured content. Recapture it before grouping.')
            continue
        parents = {key for a in node.select('a[href]')
                   if (key := link_key(a, item)) and key.startswith('gs:')}
        if len(parents) != 1:
            hold(item, 'Photo gallery is missing or ambiguous. Resolve its parent gallery before importing.')
            continue
        parent = parents.pop()
        details[parent].append(item)
        full = node.select('#fgzvet img#zvetsenina, #fgzvet .fotobig .obr img')
        urls = {canonical(img.get('src', ''), item['url']) for img in full}
        urls.discard(None)
        if len(urls) != 1:
            hold(item, 'Full-size photo is missing or ambiguous. Recapture its detail before importing.')
            continue
        url = urls.pop()
        if source_key(url) not in images:
            hold(item, 'Full-size photo was not captured as an image. Recapture the missing image before importing.')
            continue
        families[_photo(item['key'])].append({
            'item': item, 'gallery': parent, 'url': url, 'image': source_key(url),
            'alt': full[0].get('alt') or item['title'],
        })

    def entries(node, item, gallery):
        """Read only actual thumbnail links, preserving their visible ordering."""
        found = []
        for anchor in node.select('a[href]'):
            image = anchor.find('img')
            family = _photo(link_key(anchor, item))
            if image is None or family is None:
                continue
            choices = families.get(family, [])
            identities = {(photo['gallery'], photo['image']) for photo in choices}
            if len(identities) != 1 or next(iter(identities))[0] != gallery:
                raise ValueError('Gallery photo detail is missing or ambiguous. Recapture all linked photo details.')
            photo = choices[0]
            caption = anchor.get_text(' ', strip=True) or photo['item']['title']
            found.append(dict(photo, caption=caption))
        if not found:
            raise ValueError('Gallery contains no captured photo links. Review its original gallery markup.')
        return found

    def render(photos):
        seen = set()
        blocks = []
        for photo in photos:
            if photo['image'] in seen:
                continue
            seen.add(photo['image'])
            blocks.append(f'![{escape(photo["alt"])}](<{photo["url"]}>)\n\n{escape(photo["caption"])}')
        return '\n\n'.join(blocks)

    for key, photos in details.items():
        gallery = records.get(key)
        if gallery is None or 'mime' in gallery:
            for photo in photos:
                hold(photo, 'Parent gallery was not captured. Recapture it before importing its photos.')
            continue
        if not _ordinary(gallery):
            for photo in photos:
                hold(photo, 'Parent gallery is not an importable article. Choose an article owner before importing its photos.')
            continue
        if any(photo.get('review_required') for photo in photos):
            reason = 'Gallery has incomplete photo details. Resolve its reviewed photos before importing.'
            hold(gallery, reason)
            for photo in photos:
                hold(photo, reason)
            continue
        gallery_body = body(gallery)
        gallery_nodes = gallery_body.select('.obrgalerie') if gallery_body else []
        if len(gallery_nodes) != 1:
            reason = 'Gallery photo list is missing or ambiguous. Review its captured markup before importing.'
            hold(gallery, reason)
            for photo in photos:
                hold(photo, reason)
            continue
        owner = gallery
        owner_node = gallery_nodes[0]
        # A gallery may have its own introduction. Its return link alone must
        # not discard prose that is absent from the enclosing article.
        remaining = gallery['content'].replace(_text(owner_node, gallery['url']), '', 1)
        for navigation in gallery_body.select('.bodkazy, .odkazy.navratove'):
            remaining = remaining.replace(_text(navigation, gallery['url']), '', 1)
        parents = {link_key(a, gallery) for a in gallery_body.select('.odkazy.navratove a[href]')}
        parents.discard(None)
        if len(parents) == 1:
            candidate = records.get(next(iter(parents)))
            if candidate and candidate['key'] in document_pages:
                reason = 'Gallery parent is excluded because it links to documents. Choose an article owner before importing its photos.'
                hold(gallery, reason)
                for photo in photos:
                    hold(photo, reason)
                continue
            if candidate and _ordinary(candidate) and not candidate['key'].startswith(('g:', 'gs:')):
                parent_body = body(candidate)
                embedded = []
                for node in parent_body.select('.obrgalerie') if parent_body else []:
                    linked = {link_key(a, candidate) for a in node.select('a[href]')}
                    if key in linked:
                        embedded.append(node)
                if len(embedded) == 1 and not remaining.strip():
                    owner, owner_node = candidate, embedded[0]
        if key in document_pages:
            reason = 'Gallery is excluded because it links to documents. Choose an article owner before importing its photos.'
            hold(gallery, reason)
            for photo in photos:
                hold(photo, reason)
            continue
        try:
            ordered = entries(owner_node, owner, key)
            ordered.extend(entries(gallery_nodes[0], gallery, key))
            # A detail can be reached from a slideshow without appearing on the
            # captured thumbnail page. Keep that image in the same gallery.
            for photo in photos:
                choices = families[_photo(photo['key'])]
                identity = {(entry['gallery'], entry['image']) for entry in choices}
                if len(identity) != 1 or next(iter(identity))[0] != key:
                    raise ValueError('Photo variants disagree about their image or gallery. Review the source variants.')
                ordered.append(dict(choices[0], caption=photo['title']))
            original = _text(owner_node, owner['url'])
            content = owner.get('gallery_content', owner['content'])
            if not original or content.count(original) != 1:
                raise ValueError('Gallery location cannot be matched uniquely in the article. Review its extracted content.')
            content = content.replace(original, render(ordered), 1)
            if len(content) > 100_000:
                raise ValueError('Combined gallery exceeds the article size limit. Split it into reviewed galleries.')
        except ValueError as error:
            hold(gallery, str(error))
            for photo in photos:
                hold(photo, str(error))
            continue
        aliases = photos + ([gallery] if owner is not gallery else [])
        for alias in aliases:
            alias['page_alias'] = owner['key']
        owner['gallery_content'] = content
        owner['gallery_members'] = sorted(set(owner.get('gallery_members', [])) | {alias['key'] for alias in aliases})
    return result
