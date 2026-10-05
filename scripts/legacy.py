#!/usr/bin/env python3
"""Capture Vyskeř's public Vismo content into a resumable, offline migration bundle.

Based on the menu/content discovery approach in Pavel's /home/pavel/dev/scrape.
This tool never runs that scraper's notification code or submits website forms.
"""
import argparse
from collections import deque
from datetime import date, datetime, timezone
from email.message import Message
import hashlib
import json
from pathlib import Path
import re
import time
from urllib.error import HTTPError
from urllib.parse import parse_qsl, quote, unquote, urlencode, urljoin, urlsplit, urlunsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener
from xml.etree import ElementTree
from zoneinfo import ZoneInfo

from bs4 import BeautifulSoup, Comment, NavigableString

ORIGIN = 'https://vysker.cz'
HOSTS = {'vysker.cz', 'www.vysker.cz'}
VERSION = 1
USER_AGENT = 'VyskerMigration/1.0 (+https://github.com/Pscheidl/vysker-modern)'
DROP_PARAMS = {'p1', 'p2', 'p3', 'n', 'sort', 'razeni', 'grafika'}
BLOCKED = ('/aa/', '/admin/', '/aspinclude/', '/vismo/formulare', '/vismo/zaslat',
           '/vismo/navstevnost', '/vismo/cookies', '/vismo/login')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def now():
    return datetime.now(timezone.utc).isoformat()


def canonical(value, base=ORIGIN + '/'):
    try:
        value = urljoin(base, value)
        parts = urlsplit(value)
        port = parts.port
    except ValueError:
        return None
    if parts.scheme not in ('http', 'https') or parts.hostname not in HOSTS:
        return None
    if parts.username or parts.password or port not in (None, 80, 443):
        return None
    path = unquote(parts.path)
    if any(ord(c) < 32 for c in path) or '\\' in path:
        return None
    query = dict(parse_qsl(parts.query, keep_blank_values=True))
    if any(k.lower().startswith(('xv', 'utm_')) for k in query):
        return None
    path = re.sub(r'/p[123]=[^/]*', '', path)
    if path.lower().startswith(BLOCKED):
        return None
    if path.lower() in ('/index.asp', ''):
        path = '/'
    query = {k.lower(): v for k, v in query.items() if k.lower() not in DROP_PARAMS}
    return urlunsplit(('https', 'vysker.cz', quote(path, safe='/=-._~'), urlencode(sorted(query.items())), ''))


def source_key(value):
    normalized = canonical(value)
    if not normalized:
        return None
    p = urlsplit(normalized)
    gallery = re.fullmatch(r'/gp/id_galerie=(\d+)', p.path)
    if gallery:
        return 'gs:' + gallery[1]
    asp = {'/vismo/dokumenty2.asp': ('id', 'd'), '/vismo/o_utvar.asp': ('id_u', 'os'),
           '/vismo/o_osoba.asp': ('id_o', 'o'), '/vismo/akce.asp': ('id', 'a'),
           '/vismo/galerie3.asp': ('id_fotopary', 'g')}
    if p.path.lower() in asp:
        param, kind = asp[p.path.lower()]
        query = dict(parse_qsl(p.query))
        if query.get(param, '').isdigit():
            return f'{kind}:{query[param]}'
    match = re.search(r'/(ms|ds|os|o|d|a|gs|g)-(\d+)(?:/|$)', p.path)
    if match:
        suffix = p.path[match.end():].strip('/')
        return f'{match[1]}:{match[2]}' + (f'/{suffix}' if suffix else '') + (f'?{p.query}' if p.query else '')
    return p.path + (f'?{p.query}' if p.query else '')


def is_asset(url):
    path = urlsplit(url).path.lower()
    return path.startswith('/assets/') or bool(re.search(r'\.(pdf|docx?|xlsx?|od[ts]|csv|txt|jpe?g|png|gif|webp|zip)$', path))


def is_page(url):
    p = urlsplit(url)
    if is_asset(url):
        return False
    if p.path == '/' or re.search(r'/(ms|ds|os|o|d|a|gs|g)-\d+', p.path):
        return True
    return p.path.startswith(('/uredni-deska/', '/dp', '/dsp', '/ap', '/gs', '/gp', '/osp', '/mapa-stranek')) or p.path.lower() in (
        '/vismo/zobraz_dok.asp', '/vismo/zobraz_dok2.asp', '/vismo/prehled.asp', '/vismo/galerie2.asp',
        '/vismo/dokumenty2.asp', '/vismo/o_utvar.asp', '/vismo/o_osoba.asp', '/vismo/akce.asp',
        '/vismo/galerie3.asp', '/vismo/isvs.asp')


def escape(text):
    return re.sub(r'([\\`*_{}\[\]<>#!|])', r'\\\1', text)


def markdown(node, base):
    if isinstance(node, Comment):
        return ''
    if isinstance(node, NavigableString):
        return escape(re.sub(r'\s+', ' ', str(node)))
    tag = node.name
    if tag in ('script', 'style', 'form', 'iframe', 'noscript', 'button', 'input', 'select'):
        return ''
    content = ''.join(markdown(child, base) for child in node.children)
    if tag == 'img':
        url = canonical(node.get('src', ''), base)
        if url and urlsplit(url).path.startswith(('/html/', '/aspinclude/')):
            return escape(node.get('alt', ''))
        return f'![{escape(node.get("alt", "Obrázek"))}](<{url}>)' if url else escape(node.get('alt', ''))
    if tag == 'a' and node.get('href'):
        try:
            href = urljoin(base, node['href'])
            parts = urlsplit(href)
            parts.port  # Invalid ports must not discard the rest of the page.
        except ValueError:
            return content
        normalized = canonical(href)
        url = normalized or href
        if parts.scheme not in ('http', 'https', 'mailto', 'tel'):
            return content
        if image := node.find('img'):
            image_url = canonical(image.get('src', ''), base)
            if image_url and urlsplit(image_url).path.startswith(('/html/', '/aspinclude/')):
                return f'[{escape(image.get("alt", "Podrobnosti"))}](<{url}>)'
            if canonical(url) and is_asset(url):
                return f'![{escape(image.get("alt", "Obrázek"))}](<{url}>)'
            return content + f' [{escape(node.get("title", "Podrobnosti"))}](<{url}>)'
        # Fragments are retained for later link review, never sent to the crawler.
        if normalized and parts.fragment:
            url += '#' + quote(unquote(parts.fragment), safe='-_')
        return f'[{content.strip() or escape(url)}](<{url}>)'
    if tag in ('strong', 'b') and content.strip():
        return f'**{content.strip()}**'
    if tag in ('em', 'i') and content.strip():
        return f'*{content.strip()}*'
    if tag == 'br':
        return '\n'
    if tag in ('h1', 'h2', 'h3', 'h4', 'h5', 'h6'):
        return '\n\n' + '#' * max(2, int(tag[1])) + ' ' + content.strip() + '\n\n'
    if tag == 'li':
        return '\n- ' + content.strip().replace('\n', '\n  ') + '\n'
    if tag == 'tr':
        cells = [markdown(c, base).strip() for c in node.find_all(['td', 'th'], recursive=False)]
        return '\n' + ' | '.join(cells) + '\n'
    if tag in ('p', 'div', 'section', 'article', 'ul', 'ol', 'table', 'dl', 'dt', 'dd', 'address'):
        return '\n\n' + content.strip() + '\n\n' if content.strip() else ''
    return content


def czech_date(value):
    match = re.search(r'(?<!\d)(\d{1,2})\.\s*(\d{1,2})\.\s*(\d{4})(?!\d)', value)
    if not match:
        return None
    try:
        return date(int(match[3]), int(match[2]), int(match[1])).isoformat()
    except ValueError:
        return None


def dates(text):
    result = {}
    # Labels may omit a colon. Require a complete date immediately after the label,
    # so a missing publication year cannot borrow the later deadline's year.
    for label, field in [(r'Vyvěšeno|Vyvěšno|Úřední deska od', 'published_on'),
                         ('Sejmuto', 'withdraw_on'), ('Zveřejněno', 'published_on'),
                         ('Lhůta do', 'deadline_on')]:
        match = re.search(r'(?:' + label + r')\s*:?\s*(\d{1,2}\.\s*\d{1,2}\.\s*\d{4})(?!\d)', text, re.I)
        if match and (value := czech_date(match[1])):
            result[field] = value
    return result


def event_timing(text):
    """Unknown times use day bounds for filtering, with explicit precision flags."""
    parts = re.fullmatch(r'\s*(\d{1,2}\.\s*\d{1,2}\.\s*\d{4})(?:\s+(\d{1,2}:\d{2}))?(?:\s*-\s*(\d{1,2}\.\s*\d{1,2}\.\s*\d{4})(?:\s+(\d{1,2}:\d{2}))?)?\s*', text)
    if not parts:
        return None
    def point(day, clock, end=False):
        day = czech_date(day)
        value = datetime.fromisoformat(day + 'T' + (clock.zfill(5) if clock else ('23:59:59' if end else '00:00:00')))
        zone = ZoneInfo('Europe/Prague')
        value = value.replace(tzinfo=zone)
        if clock and (value.utcoffset() != value.replace(fold=1).utcoffset()
                      or value.astimezone(timezone.utc).astimezone(zone) != value):
            raise ValueError('Ambiguous or nonexistent legacy event time')
        return value.isoformat()
    try:
        start, end = point(parts[1], parts[2]), point(parts[3] or parts[1], parts[4], True)
        if datetime.fromisoformat(end) < datetime.fromisoformat(start):
            return None
        return dict(starts_at=start, ends_at=end, start_time_known=bool(parts[2]), end_time_known=bool(parts[4]), end_date_known=bool(parts[3]), original=text)
    except (ValueError, TypeError):
        return None


def extract(raw, url):
    soup = BeautifulSoup(raw, 'html.parser')
    body = soup.select_one('#stred') or (soup.select_one('#uvod') if urlsplit(url).path == '/' else None)
    if body is None:
        raise ValueError('Missing #stred content, manual extraction required')
    heading = body.select_one('#zahlavi h2, h1, h2')
    title = heading.get_text(' ', strip=True) if heading else soup.title.get_text(' ', strip=True).split(': obec')[0]
    found = []
    for area in [body, *soup.select('#menu, #menu1')]:
        for a in area.select('a[href]'):
            if target := canonical(a['href'], url):
                if is_page(target):
                    # Calendar date navigation is unbounded, actual events come from the sitemap.
                    if not re.search(r'[?&](datum|mesic|rok|date)=', target):
                        found.append(target)
    original_text = body.get_text(' ', strip=True).replace('\xa0', ' ')
    metadata = dates(original_text) if source_key(url).startswith('d:') else {}
    event = None
    if soup.select_one('#akce'):
        when = body.select_one('#akce dl dd')
        if when:
            event = event_timing(when.get_text(' ', strip=True).replace('\xa0', ' '))
            if event:
                where = next((dt.find_next_sibling('dd') for dt in body.select('#akce dt') if dt.get_text(strip=True) == 'Kde:'), None)
                event['location'] = where.get_text(' ', strip=True) if where else ''
    created = re.search(r'Vytvořeno\s*/\s*změněno:\s*(\d+\.\s*\d+\.\s*\d{4})\s*/\s*(\d+\.\s*\d+\.\s*\d{4})', original_text)
    if created:
        metadata.update(created_on=czech_date(created[1]), modified_on=czech_date(created[2]))
    for unwanted in body.select('script, style, form, .vol-sdileni, .zobrazeno, .dpopis, .sf, #zahlavi, #zahlavi2, #kalakci, #akce > h3.cist, .akce-podle-data, .kalendar, .calendar, .map_not_shown, .vismo-cookies, .cookie-consent, .zalozky'):
        unwanted.decompose()
    assets = {}
    for element in body.select('a[href], img[src]'):
        target = canonical(element.get('href', element.get('src', '')), url)
        if not target or not is_asset(target):
            continue
        label = element.get_text(' ', strip=True) or element.get('alt') or 'Příloha'
        parent = element.find_parent('li')
        evidence = parent.get_text(' ', strip=True) if parent else ''
        assets.setdefault(target, {'url': target, 'title': label, 'dates': dates(evidence), 'evidence': evidence})
    # Remove calendar navigation widgets without dropping article/event descriptions.
    for table in body.select('table'):
        if 'týden' in table.get_text(' ', strip=True).lower() and table.select('a[href*="datum="]'):
            table.decompose()
    content = re.sub(r'\n[ \t]+', '\n', markdown(body, url))
    content = re.sub(r'\n{3,}', '\n\n', content).strip()
    kind = 'page'
    if re.search(r'(Vyvěšeno|Úřední deska od)\s*:?', original_text, re.I) and source_key(url).startswith('d:'):
        kind = 'notice'
    warnings = []
    if re.search(r'/a-\d+', url) and not event:
        warnings.append('Legacy calendar entry kept as a page. Review event times before converting to structured calendar.')
    if not content:
        warnings.append('Empty extracted content')
    return dict(key=source_key(url), url=url, title=title, content=content, kind=kind,
                dates=metadata, event=event, assets=list(assets.values()), links=sorted(set(found)), warnings=warnings)


class SafeRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not canonical(newurl):
            raise ValueError('Redirect leaves the authorized website')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


class Capture:
    def __init__(self, root, delay=.4, verbose=True):
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.delay = delay
        self.verbose = verbose
        self.opener = build_opener(SafeRedirect())

    def fetch(self, url, limit):
        cache = self.root / 'responses' / (digest(url.encode()) + '.json')
        if cache.exists():
            result = json.loads(cache.read_text())
            return result, self.blob(result['sha256'])
        time.sleep(self.delay)
        for attempt in range(3):
            try:
                with self.opener.open(Request(url, headers={'User-Agent': USER_AGENT}), timeout=40) as response:
                    body = response.read(limit + 1)
                    if len(body) > limit:
                        raise ValueError(f'Response exceeds {limit} bytes')
                    result = dict(url=url, final_url=response.url, captured_at=now(),
                                  content_type=response.headers.get('Content-Type', ''),
                                  disposition=response.headers.get('Content-Disposition', ''),
                                  sha256=digest(body), size=len(body))
                    obj = self.root / 'objects' / result['sha256']
                    obj.parent.mkdir(exist_ok=True)
                    obj.write_bytes(body)
                    cache.parent.mkdir(exist_ok=True)
                    cache.write_text(json.dumps(result, ensure_ascii=False, indent=2))
                    return result, body
            except HTTPError as e:
                if e.code < 500 and e.code != 429:
                    raise
            except (TimeoutError, OSError):
                if attempt == 2:
                    raise
            time.sleep(2 ** (attempt + 1))
        raise ValueError('Download failed after three attempts')

    def blob(self, sha):
        if not re.fullmatch('[a-f0-9]{64}', sha):
            raise ValueError('Invalid object hash')
        path = self.root / 'objects' / sha
        if path.resolve().parent != (self.root / 'objects').resolve():
            raise ValueError('Object outside bundle')
        data = path.read_bytes()
        if digest(data) != sha:
            raise ValueError('Object checksum mismatch')
        return data

    def crawl(self, max_pages=1500, max_assets=5000):
        started = now()
        unavailable_resources = []
        sitemap_url = ORIGIN + '/vismo/sitemap.asp'
        try:
            _, sitemap = self.fetch(sitemap_url, 5_000_000)
            urls = [n.text for n in ElementTree.fromstring(sitemap).iter() if n.tag.endswith('}loc')]
        except HTTPError as error:
            if error.code != 404:
                raise
            unavailable_resources.append(dict(url=sitemap_url, final_url=error.url,
                kind='sitemap', status=404, checked_at=now(), error=str(error)))
            urls = []
        queue = deque([ORIGIN + '/', ORIGIN + '/uredni-deska/2', ORIGIN + '/uredni-deska/1/archiv=1'] + urls)
        pages, aliases, assets, errors, skipped = {}, {}, {}, [], set()
        while queue:
            original = queue.popleft()
            url = canonical(original)
            if not url or not is_page(url):
                skipped.add(original)
                continue
            key = source_key(url)
            aliases[original] = key
            aliases[url] = key
            if key in pages:
                continue
            if len(pages) >= max_pages:
                errors.append(dict(url=url, error='Page limit reached. Increase --max-pages and resume.'))
                break
            try:
                response, raw = self.fetch(url, 5_000_000)
                item = extract(raw, url)
                item['capture'] = response
                pages[key] = item
                for ref in item['assets']:
                    assets.setdefault(ref['url'], dict(ref, parents=[]))['parents'].append(key)
                queue.extend(item['links'])
                if self.verbose:
                    print(f'page {len(pages)} assets {len(assets)} {url}', flush=True)
            except Exception as error:
                pages[key] = dict(key=key, url=url, error=str(error))
                if isinstance(error, HTTPError) and error.code == 404:
                    unavailable_resources.append(dict(url=url, final_url=error.url, key=key,
                        kind='page', status=404, checked_at=now(), error=str(error)))
                else:
                    errors.append(dict(url=url, error=f'{type(error).__name__}: {error}'))
        for i, (url, asset) in enumerate(assets.items()):
            if i >= max_assets:
                asset['error'] = 'Asset limit reached'
                errors.append(dict(url=url, error=asset['error']))
                continue
            try:
                response, raw = self.fetch(url, 30 * 1024 * 1024)
                asset['capture'] = response
                message = Message()
                message['content-disposition'] = response['disposition']
                extended = re.search(r"filename\*\s*=\s*UTF-8''([^;]+)", response['disposition'], re.I)
                filename = unquote(extended[1].strip(' \"')) if extended else message.get_filename()
                mime, extension = file_type(raw)
                asset['name'] = re.sub(r'[\\/\x00-\x1f]', '_', filename or asset['title'])[:140]
                original_extension = asset['name'].rsplit('.', 1)[-1].lower()
                if (extension == 'zip' and original_extension in ('docx', 'xlsx', 'odt', 'ods', 'zip')
                        or extension == 'ole' and original_extension in ('doc', 'xls')):
                    extension = original_extension
                if not asset['name'].lower().endswith('.' + extension):
                    asset['name'] += '.' + extension
                asset['mime'] = mime
                if self.verbose:
                    print(f'asset {i+1}/{len(assets)} {len(raw)} bytes', flush=True)
            except Exception as error:
                asset['error'] = f'{type(error).__name__}: {error}'
                if isinstance(error, HTTPError) and error.code == 404:
                    unavailable_resources.append(dict(url=url, final_url=error.url, key=source_key(url),
                        kind='asset', status=404, checked_at=now(), error=asset['error']))
                else:
                    errors.append(dict(url=url, error=asset['error']))
        manifest = dict(version=VERSION, origin=ORIGIN, started_at=started, completed_at=now(),
                        pages=list(pages.values()), assets=list(assets.values()), aliases=aliases,
                        errors=errors, unavailable_resources=unavailable_resources,
                        skipped=sorted(skipped), complete=not errors)
        (self.root / 'manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2))
        summary = dict(pages=len(pages), assets=len(assets), errors=len(errors),
                       unavailable_resources=len(unavailable_resources),
                       warnings=sum(len(p.get('warnings', [])) for p in pages.values()),
                       bytes=sum(a.get('capture', {}).get('size', 0) for a in assets.values()),
                       complete=not errors)
        (self.root / 'summary.json').write_text(json.dumps(summary, indent=2))
        if self.verbose:
            print(json.dumps(summary, indent=2))
        return manifest


def file_type(data):
    for magic, mime, ext in [(b'%PDF-', 'application/pdf', 'pdf'), (b'\x89PNG\r\n\x1a\n', 'image/png', 'png'),
                             (b'\xff\xd8\xff', 'image/jpeg', 'jpg'), (b'GIF8', 'image/gif', 'gif'),
                             (b'{\\rtf', 'application/rtf', 'rtf'),
                             (b'PK\x03\x04', 'application/octet-stream', 'zip'),
                             (b'\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1', 'application/octet-stream', 'ole')]:
        if data.startswith(magic):
            return mime, ext
    if data[:4] == b'RIFF' and data[8:12] == b'WEBP':
        return 'image/webp', 'webp'
    raise ValueError('Unrecognized file signature. Preserve captured object for manual review.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['capture'])
    parser.add_argument('--bundle', required=True)
    parser.add_argument('--delay', type=float, default=.4)
    parser.add_argument('--max-pages', type=int, default=1500)
    parser.add_argument('--max-assets', type=int, default=5000)
    args = parser.parse_args()
    if args.delay < .1 or args.max_pages < 1 or args.max_assets < 1:
        parser.error('Use delay >= 0.1 and positive resource limits')
    result = Capture(args.bundle, args.delay).crawl(args.max_pages, args.max_assets)
    if not result['complete']:
        raise SystemExit(2)


if __name__ == '__main__':
    main()
