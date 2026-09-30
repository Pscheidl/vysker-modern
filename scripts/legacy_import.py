#!/usr/bin/env python3
"""Validate and import an offline Vismo bundle, with no SMTP or public publishing by default."""
import argparse
from datetime import date
import json
from pathlib import Path
import re
import unicodedata

from legacy import Capture, VERSION, canonical, digest, file_type, now, source_key
from postgres import connect


def fingerprint(item):
    # HTTP fetch time and the live website clock are not editorial changes.
    fields = {k: item.get(k) for k in ('key', 'url', 'title', 'content', 'kind', 'dates', 'event', 'assets', 'name', 'mime')}
    if item.get('capture') and 'mime' in item:
        fields['sha256'] = item['capture']['sha256']
    return digest(json.dumps(fields, sort_keys=True, ensure_ascii=False).encode())


def slug(item):
    title = unicodedata.normalize('NFKD', item['title']).encode('ascii', 'ignore').decode().lower()
    title = re.sub('[^a-z0-9]+', '-', title).strip('-')[:45]
    return 'puvodni-' + title + '-' + digest(item['key'].encode())[:12]


def load_bundle(root, allow_incomplete=False):
    capture = Capture(root, 0)
    manifest = json.loads((Path(root) / 'manifest.json').read_text())
    if manifest['version'] != VERSION or manifest['origin'] != 'https://vysker.cz':
        raise ValueError('Unsupported bundle version or origin')
    if not manifest['complete'] and not allow_incomplete:
        raise ValueError('Capture is incomplete. Review errors before using --allow-incomplete')
    keys = set()
    items = []
    for item in manifest['pages'] + manifest['assets']:
        if item.get('error'):
            continue
        key = source_key(item['url'])
        if not key or key in keys:
            raise ValueError('Duplicate or invalid source identity')
        keys.add(key)
        raw = capture.blob(item['capture']['sha256'])
        if len(raw) != item['capture']['size']:
            raise ValueError('Object size mismatch')
        if 'mime' in item:
            mime, _ = file_type(raw)
            if mime != item['mime'] or not (0 < len(raw) <= 30 * 1024 * 1024):
                raise ValueError('Invalid attachment')
            if len(item['name']) > 180 or re.search(r'[\\/\x00-\x1f]', item['name']):
                raise ValueError('Unsafe filename')
        elif len(item['content']) > 100_000:
            item = dict(item, review_required='Oversized body, not imported')
        for value in item.get('dates', {}).values():
            date.fromisoformat(value)
        items.append(dict(item, key=key))
    return manifest, items, capture


def prepare(conn, root, allow_incomplete=False):
    manifest, items, capture = load_bundle(root, allow_incomplete)
    known = dict(conn.execute('SELECT source_key,fingerprint FROM legacy_sources').fetchall())
    result = {'new': [], 'unchanged': [], 'conflicts': [], 'review': [],
              'capture_errors': manifest['errors'], 'source_summary': {'pages': len(manifest['pages']), 'assets': len(manifest['assets'])}}
    for item in items:
        key = item['key']
        if item.get('review_required'):
            result['review'].append({'key': key, 'reason': item['review_required']})
        elif key in known:
            result['unchanged' if known[key] == fingerprint(item) else 'conflicts'].append(key)
        else:
            result['new'].append(key)
        for reason in item.get('warnings', []):
            result['review'].append({'key': key, 'reason': reason})
    return result, items, capture


def audit(conn, kind, id, timestamp):
    conn.execute('INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id) VALUES (%s,%s,%s,%s)',
                 (timestamp, 'legacy_imported', kind, id))


def import_bundle(conn, root, publish_content=False, allow_incomplete=False, page_map=None):
    """Caller owns transaction. Conflicts abort the entire batch before writes."""
    conn.execute("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))")
    report, items, capture = prepare(conn, root, allow_incomplete)
    if report['conflicts']:
        raise ValueError('Changed source records require manual reconciliation: ' + ', '.join(report['conflicts'][:20]))
    new = set(report['new'])
    page_map = page_map or {}
    if any(not re.fullmatch('[a-z0-9]+(?:-[a-z0-9]+)*', value) or len(value)>80 for value in page_map.values()):
        raise ValueError('Invalid mapped page slug')
    if len(set(page_map.values())) != len(page_map):
        raise ValueError('Duplicate mapped page slug')
    timestamp = now()
    # Preview publication is deliberately limited to ordinary content. Notices need review.
    routes = dict(conn.execute('SELECT source_key,destination FROM legacy_sources').fetchall())
    notice_keys = {item['key'] for item in items if item.get('kind') == 'notice'}
    new_pages = []
    for item in items:
        if item['key'] not in new:
            continue
        page_id = document_id = attachment_id = notice_id = event_id = None
        metadata = {k: item.get(k) for k in ('dates', 'event', 'warnings', 'parents', 'evidence', 'capture')}
        metadata['original_title'] = item['title']
        if 'mime' in item:
            owners = conn.execute('SELECT notice_id FROM legacy_sources WHERE source_key=ANY(%s) AND notice_id IS NOT NULL ORDER BY source_key', (item.get('parents', []),)).fetchall()
            if owners:
                notice_id = owners[0][0]
                if len(owners) > 1:
                    report['review'].append({'key': item['key'], 'reason': 'Shared notice attachment. Review ownership before publishing.'})
            else:
                description = 'Převedeno z původního webu.'
                if posted := item.get('dates', {}).get('published_on'):
                    description += ' Původní web uváděl zveřejnění dne ' + date.fromisoformat(posted).strftime('%d. %m. %Y') + '.'
                document_id = conn.execute("INSERT INTO documents(title,description,status,created_at,published_at) VALUES (%s,%s,%s,%s,NULL) RETURNING id",
                    (item['title'][:300], description,
                     'published' if publish_content and not notice_keys.intersection(item.get('parents', [])) else 'draft', timestamp)).fetchone()[0]
            raw = capture.blob(item['capture']['sha256'])
            attachment_id = conn.execute('INSERT INTO attachments(document_id,notice_id,name,content_type,size_bytes,data,sha256) VALUES (%s,%s,%s,%s,%s,%s,%s) RETURNING id',
                (document_id, notice_id, item['name'], item['mime'], len(raw), raw, digest(raw))).fetchone()[0]
            destination = f'/api/v1/attachments/{attachment_id}'
            audit(conn, 'attachment', attachment_id, timestamp)
            if document_id:
                audit(conn, 'document', document_id, timestamp)
        elif item['kind'] == 'notice' and item.get('dates', {}).get('published_on'):
            # No inferred posting time, no default +15 days, no fabricated publication event.
            notice_id = conn.execute("INSERT INTO notices(title,description,published_on,withdraw_on,status,review_json) VALUES (%s,%s,%s,%s,'draft',%s) RETURNING id",
                (item['title'][:300], item['content'], item['dates']['published_on'], item['dates'].get('withdraw_on'),
                 json.dumps({'original_reference': item['url']}))).fetchone()[0]
            destination = f'/uredni-deska/{notice_id}'
            audit(conn, 'notice', notice_id, timestamp)
            report['review'].append({'key': item['key'], 'reason': 'Notice remains a draft. Check dates, attachments and publication rules before cutover.'})
        elif item.get('event') and len(item['content']) <= 10_000:
            event = item['event']
            event_id = conn.execute('INSERT INTO events(title,description,location,starts_at,ends_at,start_time_known,end_time_known,end_date_known,published,updated_at) VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s) RETURNING id',
                (item['title'][:200], item['content'], event['location'][:300], event['starts_at'], event['ends_at'],
                 event['start_time_known'], event['end_time_known'], event['end_date_known'], publish_content, timestamp)).fetchone()[0]
            destination = f'/kalendar/{event_id}'
            new_pages.append((event_id, None, item))
            audit(conn, 'event', event_id, timestamp)
        else:
            page_slug = page_map.get(item['key'], slug(item))
            page_id = conn.execute('INSERT INTO pages(slug,title,content,published,updated_at) VALUES (%s,%s,%s,%s,%s) RETURNING id',
                (page_slug, item['title'][:200], item['content'], publish_content and item['kind'] != 'notice', timestamp)).fetchone()[0]
            if item['kind'] == 'notice':
                report['review'].append({'key': item['key'], 'reason': 'Notice without a reliable posting date retained as a private content page.'})
            destination = f'/stranky/{page_slug}'
            new_pages.append((page_id, page_slug, item))
            audit(conn, 'page', page_id, timestamp)
        routes[item['key']] = destination
        conn.execute('INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,page_id,notice_id,document_id,attachment_id,event_id,destination) VALUES (%s,%s,%s,%s,%s,%s::jsonb,%s,%s,%s,%s,%s,%s)',
            (item['key'], item['url'], fingerprint(item), item['capture']['captured_at'], timestamp,
             json.dumps(metadata, ensure_ascii=False), page_id, notice_id, document_id, attachment_id, event_id, destination))
    unresolved = set()
    for page_id, page_slug, item in new_pages:
        def rewrite(match):
            image, label, url = match.groups()
            target = routes.get(source_key(url))
            if target:
                if '#' in url:
                    report['review'].append({'key': item['key'], 'reason': 'Review legacy fragment: ' + url})
                if image:
                    # Only server-verified local images can be embedded by the renderer.
                    file = next((a for a in items if a['key'] == source_key(url) and 'mime' in a), None)
                    if file and file['mime'].startswith('image/'):
                        target = target.replace('/attachments/', '/legacy-media/')
                    else:
                        image = ''
                return f'{image}[{label}](<{target}>)'
            if canonical(url):
                unresolved.add(url)
            # Do not embed anything from the original origin.
            return f'[{label}](<{url}>)'
        content = re.sub(r'(!?)\[([^\n]*?)\]\(<([^>]+)>\)', rewrite, item['content'])
        if page_slug is None:
            conn.execute('UPDATE events SET description=%s WHERE id=%s', (content, page_id))
        else:
            conn.execute('UPDATE pages SET content=%s WHERE id=%s', (content, page_id))
            conn.execute('INSERT INTO page_revisions(page_id,version,title,content,slug,published,saved_at) SELECT id,version,title,content,slug,published,updated_at FROM pages WHERE id=%s', (page_id,))
    report['unresolved_internal_links'] = sorted(unresolved)
    report['published_content'] = publish_content
    report['imported_at'] = timestamp
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['plan', 'import'])
    parser.add_argument('--bundle', required=True)
    parser.add_argument('--report', required=True)
    parser.add_argument('--allow-incomplete', action='store_true')
    parser.add_argument('--page-map', help='JSON mapping of source keys to reviewed page slugs')
    parser.add_argument('--publish-content', action='store_true', help='Publish ordinary pages/files for a reviewed preview. Notices stay drafts.')
    args = parser.parse_args()
    with connect() as conn:
        if args.command == 'plan':
            conn.execute('SET TRANSACTION READ ONLY')
            report, _, _ = prepare(conn, args.bundle, args.allow_incomplete)
        else:
            page_map = json.loads(Path(args.page_map).read_text()) if args.page_map else None
            report = import_bundle(conn, args.bundle, args.publish_content, args.allow_incomplete, page_map)
    Path(args.report).write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps({k: len(report[k]) for k in ('new', 'unchanged', 'conflicts', 'review')}, indent=2))


if __name__ == '__main__':
    main()
