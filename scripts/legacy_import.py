#!/usr/bin/env python3
"""Validate and import an offline Vismo bundle, with no SMTP or public publishing by default."""
import argparse
from datetime import date
import json
from pathlib import Path
import re
import unicodedata

from legacy import Capture, VERSION, canonical, digest, file_type, now, source_key
from legacy_scope import notice_sections, notice_values, require_local_preview, source_dates
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


def prepare(conn, root, allow_incomplete=False, classify_navigation=False):
    manifest, items, capture = load_bundle(root, allow_incomplete)
    known = {key: (signature, metadata) for key, signature, metadata in conn.execute(
        'SELECT source_key,fingerprint,metadata FROM legacy_sources')}
    result = {'new': [], 'unchanged': [], 'conflicts': [], 'review': [],
              'capture_errors': manifest['errors'], 'source_summary': {'pages': len(manifest['pages']), 'assets': len(manifest['assets'])}}
    sections = notice_sections(items, capture) if classify_navigation else {}
    if classify_navigation:
        result['notice_sections'] = sections
        result['classification'] = {'notice': 0, 'document': 0}
        owners = dict(conn.execute("SELECT source_key,notice_id IS NOT NULL FROM legacy_sources WHERE attachment_id IS NOT NULL"))
        for item in items:
            if 'mime' not in item:
                continue
            board = bool(set(item.get('parents', [])) & sections.keys())
            result['classification']['notice' if board else 'document'] += 1
            if item['key'] in owners and owners[item['key']] != board:
                result['conflicts'].append(item['key'])
    for item in items:
        key = item['key']
        if item.get('review_required'):
            result['review'].append({'key': key, 'reason': item['review_required']})
        if key in known:
            signature, metadata = known[key]
            same = signature == fingerprint(item) and metadata.get('evidence') == item.get('evidence')
            result['unchanged' if same else 'conflicts'].append(key)
        elif not item.get('review_required'):
            result['new'].append(key)
        for reason in item.get('warnings', []):
            result['review'].append({'key': key, 'reason': reason})
    result['conflicts'] = sorted(set(result['conflicts']))
    result['unchanged'] = [key for key in result['unchanged'] if key not in result['conflicts']]
    return result, items, capture


def queue_changes(conn, report, items, capture, timestamp):
    conflicts = set(report['conflicts'])
    sections = report.get('notice_sections', {})
    for item in items:
        if item['key'] not in conflicts:
            continue
        proposal = dict(item)
        if 'notice_sections' in report and 'mime' in item:
            proposal['notice_owner'] = bool(set(item.get('parents', [])) & sections.keys())
        # Ownership changes need review even when the file bytes are unchanged.
        signature = digest(json.dumps([fingerprint(item), item.get('evidence'), proposal.get('notice_owner')]).encode())
        conn.execute('''INSERT INTO legacy_sync_reviews
            (source_key,fingerprint,proposal,source_data,first_seen_at,last_seen_at)
            VALUES (%s,%s,%s::jsonb,%s,%s,%s)
            ON CONFLICT (source_key) DO UPDATE SET
                first_seen_at=CASE WHEN legacy_sync_reviews.fingerprint=EXCLUDED.fingerprint
                    THEN legacy_sync_reviews.first_seen_at ELSE EXCLUDED.first_seen_at END,
                reviewed_at=CASE WHEN legacy_sync_reviews.fingerprint=EXCLUDED.fingerprint
                    THEN legacy_sync_reviews.reviewed_at ELSE NULL END,
                fingerprint=EXCLUDED.fingerprint,proposal=EXCLUDED.proposal,
                source_data=EXCLUDED.source_data,last_seen_at=EXCLUDED.last_seen_at''',
            (item['key'], signature, json.dumps(proposal, ensure_ascii=False),
             capture.blob(item['capture']['sha256']), timestamp, timestamp))
    report['pending_reviews'] = conn.execute(
        'SELECT count(*) FROM legacy_sync_reviews WHERE reviewed_at IS NULL').fetchone()[0]


def audit(conn, kind, id, timestamp):
    conn.execute('INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id) VALUES (%s,%s,%s,%s)',
                 (timestamp, 'legacy_imported', kind, id))


def import_bundle(conn, root, publish_content=False, allow_incomplete=False, page_map=None,
                  classify_navigation=False, preview_notices=False, notice_map=None, attachment_ids=None,
                  sync=False):
    """Caller owns transaction. Sync queues conflicts and imports only new identities."""
    conn.execute("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))")
    if preview_notices:
        require_local_preview()
        if not classify_navigation:
            raise ValueError('Preview notices require navigation classification')
    report, items, capture = prepare(conn, root, allow_incomplete, classify_navigation)
    if report['conflicts'] and not sync:
        raise ValueError('Changed source records require manual reconciliation: ' + ', '.join(report['conflicts'][:20]))
    new = set(report['new'])
    page_map = page_map or {}
    if any(not re.fullmatch('[a-z0-9]+(?:-[a-z0-9]+)*', value) or len(value)>80 for value in page_map.values()):
        raise ValueError('Invalid mapped page slug')
    if len(set(page_map.values())) != len(page_map):
        raise ValueError('Duplicate mapped page slug')
    timestamp = now()
    sections = report.get('notice_sections', {})
    categories = dict(conn.execute('SELECT name,id FROM categories'))
    attachment_ids = attachment_ids or {}
    # Ordinary content and the isolated notice rehearsal have separate publication flags.
    routes = dict(conn.execute('SELECT source_key,destination FROM legacy_sources').fetchall())
    notice_keys = {item['key'] for item in items if item.get('kind') == 'notice'}
    new_pages = []
    for item in items:
        if item['key'] not in new:
            continue
        page_id = document_id = attachment_id = notice_id = event_id = None
        metadata = {k: item.get(k) for k in ('dates', 'event', 'warnings', 'parents', 'evidence', 'capture')}
        metadata['original_title'] = item['title']
        notice_metadata = None
        if 'mime' in item:
            on_board = bool(set(item.get('parents', [])) & sections.keys())
            owners = conn.execute('SELECT notice_id FROM legacy_sources WHERE source_key=ANY(%s) AND notice_id IS NOT NULL ORDER BY source_key', (item.get('parents', []),)).fetchall() if not classify_navigation or on_board else []
            if owners:
                notice_id = owners[0][0]
                if len(owners) > 1:
                    report['review'].append({'key': item['key'], 'reason': 'Shared notice attachment. Review ownership before publishing.'})
            elif classify_navigation and on_board:
                values = notice_values(item, categories, sections, notice_map or {}, preview_notices)
                notice_id = conn.execute('''INSERT INTO notices(title,description,category_id,published_on,
                    withdraw_on,status,retain_attachments,review_json,created_at,updated_at)
                    VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s) RETURNING id''',
                    (item['title'][:300], values['description'], values['category_id'], values['published_on'],
                     values['withdraw_on'], values['status'], preview_notices,
                     json.dumps(values['review'], ensure_ascii=False), timestamp, timestamp)).fetchone()[0]
                notice_metadata = values['metadata']
                metadata.update(notice_metadata)
                audit(conn, 'notice', notice_id, timestamp)
                for issue in values['issues']:
                    report['review'].append({'key': item['key'], 'reason': issue})
            else:
                description = 'Převedeno z původního webu.'
                posted = source_dates(item).get('published_on')
                if posted:
                    description += ' Původní web uváděl zveřejnění dne ' + date.fromisoformat(posted).strftime('%d. %m. %Y') + '.'
                document_id = conn.execute("INSERT INTO documents(title,description,status,created_at,published_at,source_published_on) VALUES (%s,%s,%s,%s,NULL,%s) RETURNING id",
                    (item['title'][:300], description,
                     'published' if publish_content and (classify_navigation or not notice_keys.intersection(item.get('parents', []))) else 'draft', timestamp,
                     posted)).fetchone()[0]
            raw = capture.blob(item['capture']['sha256'])
            columns, arguments = '', ()
            if item['key'] in attachment_ids:
                columns, arguments = 'id,', (attachment_ids[item['key']],)
            attachment_id = conn.execute('INSERT INTO attachments(' + columns + 'document_id,notice_id,name,content_type,size_bytes,data,sha256) VALUES (' + ('%s,' if columns else '') + '%s,%s,%s,%s,%s,%s,%s) RETURNING id',
                arguments + (document_id, notice_id, item['name'], item['mime'], len(raw), raw, digest(raw))).fetchone()[0]
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
        if notice_metadata is not None:
            conn.execute('''INSERT INTO legacy_notice_imports(source_key,notice_id,imported_at,preview_published,metadata)
                VALUES (%s,%s,%s,%s,%s::jsonb)''',
                (item['key'], notice_id, timestamp, preview_notices, json.dumps(notice_metadata, ensure_ascii=False)))
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
    if sync:
        queue_changes(conn, report, items, capture, timestamp)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['plan', 'import', 'sync'])
    parser.add_argument('--bundle', required=True)
    parser.add_argument('--report', required=True)
    parser.add_argument('--allow-incomplete', action='store_true')
    parser.add_argument('--page-map', help='JSON mapping of source keys to reviewed page slugs')
    parser.add_argument('--classify-navigation', action='store_true', help='Import files under the original Úřední deska breadcrumb exclusively as notices.')
    parser.add_argument('--notice-map', help='Optional JSON mapping of source section keys to notice categories')
    parser.add_argument('--preview-notices', action='store_true', help='Make imported notices visible only in an isolated local preview.')
    parser.add_argument('--publish-content', action='store_true', help='Publish new ordinary pages/files/events. Notices stay drafts.')
    args = parser.parse_args()
    with connect() as conn:
        if args.command == 'plan':
            conn.execute('SET TRANSACTION READ ONLY')
            report, _, _ = prepare(conn, args.bundle, args.allow_incomplete, args.classify_navigation)
        else:
            page_map = json.loads(Path(args.page_map).read_text()) if args.page_map else None
            notice_map = json.loads(Path(args.notice_map).read_text()) if args.notice_map else None
            report = import_bundle(conn, args.bundle, args.publish_content, args.allow_incomplete, page_map,
                                   args.classify_navigation, args.preview_notices, notice_map,
                                   sync=args.command == 'sync')
    Path(args.report).write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps({k: len(report[k]) for k in ('new', 'unchanged', 'conflicts', 'review')}, indent=2))


if __name__ == '__main__':
    main()
