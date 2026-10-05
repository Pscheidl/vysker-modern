#!/usr/bin/env python3
"""Cron entry point for additive legacy synchronization and operator review reports."""
import argparse
from html import escape
import json
import os
from pathlib import Path
import re
import tempfile
import time
from urllib.parse import quote, urlsplit

from psycopg.rows import dict_row

from legacy import Capture, canonical, digest, file_type, now
from legacy_import import import_bundle
from postgres import connect

SYNC_LOCK = "hashtextextended(current_schema() || ':vysker-legacy-sync', 0)"


def write_atomic(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix='.report-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def write_json(path, value):
    write_atomic(path, json.dumps(value, ensure_ascii=False, indent=2).encode())


def export_reviews(conn, output, base_url=''):
    """Export the persistent queue, including changes missing from the latest crawl."""
    output = Path(output)
    if base_url:
        url = urlsplit(base_url)
        if (url.scheme not in ('https', 'http') or not url.hostname or url.username or url.password
                or url.path not in ('', '/') or url.query or url.fragment):
            raise ValueError('base-url must be an HTTP(S) website origin without credentials')
    # Stream attachment bytes one record at a time even when many files changed.
    with conn.transaction(), conn.cursor(name='legacy_review_export', row_factory=dict_row) as cursor:
        cursor.itersize = 1
        cursor.execute('''SELECT r.source_key,r.fingerprint,r.proposal,r.source_data,
            r.first_seen_at,r.last_seen_at,s.destination,
            COALESCE(p.title,e.title,d.title,n.title) AS current_title,
            COALESCE(p.content,e.description,d.description,n.description) AS current_content
            FROM legacy_sync_reviews r JOIN legacy_sources s USING (source_key)
            LEFT JOIN pages p ON p.id=s.page_id LEFT JOIN events e ON e.id=s.event_id
            LEFT JOIN documents d ON d.id=s.document_id LEFT JOIN notices n ON n.id=s.notice_id
            WHERE r.reviewed_at IS NULL ORDER BY r.first_seen_at,r.source_key''')
        reviews = []
        sections = []
        for row in cursor:
            raw = bytes(row.pop('source_data'))
            proposal = row['proposal']
            attachment = ''
            if 'mime' in proposal:
                # Only verified imported file types are exported, never active source HTML.
                file = 'review-files/' + digest(raw) + '.' + file_type(raw)[1]
                write_atomic(output / file, raw)
                row['proposed_file'] = file
                attachment = f'<p><a href="{escape(quote(file), quote=True)}" download="{escape(proposal["name"], quote=True)}">Stáhnout změněnou přílohu</a></p>'
            target = base_url.rstrip('/') + row['destination'] if base_url else ''
            link = (f'<a href="{escape(target, quote=True)}">Současná položka</a>'
                    if target else escape(row['destination']))
            sections.append(f'''<article>
<h2>{escape(proposal['title'])}</h2>
<p>{escape(row['source_key'])} | {link} |
<a href="{escape(proposal['url'], quote=True)}">Původní web</a></p>
<p>Poprvé zachyceno: {escape(row['first_seen_at'])}<br>
Naposledy zachyceno: {escape(row['last_seen_at'])}</p>
<h3>Současný obsah</h3><pre>{escape(row['current_title'] or '')}\n{escape(row['current_content'] or '')}</pre>
<h3>Změna z původního webu</h3><pre>{escape(proposal.get('gallery_content', proposal.get('content', proposal.get('evidence', ''))))}</pre>
{attachment}<details><summary>Podklady a identifikátor pro potvrzení kontroly</summary>
<pre>{escape(json.dumps(row, ensure_ascii=False, indent=2))}</pre></details>
</article>''')
            reviews.append(row)
    write_json(output / 'reviews.json', reviews)
    html = '''<!doctype html><html lang="cs"><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Změny původního webu ke kontrole</title>
<style>body{font:18px/1.5 system-ui,sans-serif;max-width:1000px;margin:2rem auto;padding:0 1rem;color:#172235}
article{border-top:2px solid #ccc;margin-top:2rem}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#f2f4f7;padding:1rem}
a{color:#184da8}</style><h1>Změny původního webu ke kontrole</h1>
<p>Stávající obsah zůstal zachován. Případné úpravy proveďte v administraci.
Potvrzení kontroly pouze uzavře návrh, obsah webu nemění.
Seznam uchovává nevyřízené návrhy i po zmizení zdrojové položky.</p>'''
    html += f'<p>Čeká na kontrolu: {len(reviews)}</p>' + ''.join(sections) + '</html>'
    write_atomic(output / 'reviews.html', html.encode())
    return len(reviews)


def acknowledge(conn, key, fingerprint):
    result = conn.execute('''UPDATE legacy_sync_reviews SET reviewed_at=%s
        WHERE source_key=%s AND fingerprint=%s AND reviewed_at IS NULL''', (now(), key, fingerprint))
    if result.rowcount != 1:
        raise ValueError('Pending proposal not found or has changed. Export reviews again.')


def imported_asset_loader(conn):
    """Index provenance cheaply, loading at most one existing attachment at a time."""
    rows = conn.execute('''SELECT s.source_url, s.metadata->'capture', s.attachment_id
        FROM legacy_sources s JOIN attachments a ON a.id=s.attachment_id
        WHERE a.data IS NOT NULL AND a.removed_at IS NULL''').fetchall()
    known = {canonical(url): (capture, attachment_id) for url, capture, attachment_id in rows
             if canonical(url) and isinstance(capture, dict)}

    def load(url):
        record = known.get(url)
        if record is None:
            return None
        capture, attachment_id = record
        # Old or incomplete provenance must not invent response headers or filenames.
        # Capture validates the original URL, byte hash, size and file signature too.
        expected_size = capture.get('size')
        expected_hash = capture.get('sha256')
        if (capture.get('url') != url or type(expected_size) is not int
                or not 0 <= expected_size <= 30 * 1024 * 1024
                or not isinstance(expected_hash, str) or not re.fullmatch('[a-f0-9]{64}', expected_hash)):
            return None
        row = conn.execute('''SELECT data, sha256, size_bytes, content_type FROM attachments
            WHERE id=%s AND data IS NOT NULL AND removed_at IS NULL
            AND size_bytes=%s AND octet_length(data)=%s AND sha256=%s''',
            (attachment_id, expected_size, expected_size, expected_hash)).fetchone()
        if row is None:
            return None
        data, sha, size, mime = row
        raw = bytes(data)
        if (sha != capture.get('sha256') or size != capture.get('size')
                or len(raw) != size or digest(raw) != sha):
            return None
        try:
            if file_type(raw)[0] != mime:
                return None
        except ValueError:
            return None
        return capture, raw

    return load


def run_sync(conn, state, *, delay=.4, max_pages=1500, max_assets=5000,
             publish_content=False, allow_incomplete=False, page_map=None, notice_map=None,
             base_url='', archive_notices=False, skip_pages=False, refresh_assets=False):
    """Use an autocommit connection so the crawl never holds a DB transaction open."""
    if not conn.autocommit:
        raise ValueError('run_sync requires an autocommit connection')
    if not conn.execute(f'SELECT pg_try_advisory_lock({SYNC_LOCK})').fetchone()[0]:
        return {'status': 'skipped', 'reason': 'Another synchronization is running'}
    state = Path(state)
    started = now()
    started_clock = time.monotonic()
    committed = False
    crawler = None
    try:
        state.mkdir(parents=True, exist_ok=True, mode=0o700)
        write_json(state / 'last-run.json', {'status': 'running', 'started_at': started})
        # Rebuild discovery each time, while validated attachment bytes survive runs.
        # Bootstrap lazily from the DB so the first optimized run also avoids downloads.
        with tempfile.TemporaryDirectory(prefix='capture-', dir=state) as bundle:
            crawler = Capture(bundle, delay, verbose=False, cache_root=state / 'capture-cache',
                refresh_assets=refresh_assets,
                asset_loader=None if refresh_assets else imported_asset_loader(conn))
            capture = crawler.crawl(max_pages, max_assets)
            write_json(state / 'capture-unavailable.json', capture.get('unavailable_resources', []))
            if not capture['complete'] and not allow_incomplete:
                write_json(state / 'capture-errors.json', capture['errors'])
                raise ValueError('Capture is incomplete. See capture-errors.json before allowing a partial import.')
            with conn.transaction():
                report = import_bundle(conn, bundle, publish_content=publish_content,
                    allow_incomplete=allow_incomplete, page_map=page_map,
                    classify_navigation=True, notice_map=notice_map, sync=True,
                    archive_notices=archive_notices, skip_pages=skip_pages)
            committed = True
        write_json(state / 'capture-errors.json', report['capture_errors'])
        report['pending_reviews'] = export_reviews(conn, state, base_url)
        report.update(status='ok', started_at=started, completed_at=now(),
            duration_seconds=round(time.monotonic() - started_clock, 3),
            download_stats=crawler.stats, refresh_assets=refresh_assets)
        write_json(state / 'last-success.json', report)
        write_json(state / 'last-run.json', report)
        return report
    except Exception as error:
        # Preserve the previous success marker, including when report export fails
        # after commit. The next import still cannot duplicate the committed items.
        write_json(state / 'last-run.json', dict(status='failed', started_at=started,
            completed_at=now(), import_committed=committed, error_type=type(error).__name__,
            duration_seconds=round(time.monotonic() - started_clock, 3),
            download_stats=crawler.stats if crawler else {}, refresh_assets=refresh_assets))
        raise
    finally:
        conn.execute(f'SELECT pg_advisory_unlock({SYNC_LOCK})')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    run = commands.add_parser('run', help='Fresh discovery, cached attachments and additive import with review queue')
    run.add_argument('--state', required=True, help='Private writable directory for reports, persistent capture cache and temporary bundles')
    run.add_argument('--delay', type=float, default=.4)
    run.add_argument('--max-pages', type=int, default=1500)
    run.add_argument('--max-assets', type=int, default=5000)
    run.add_argument('--allow-incomplete', action='store_true')
    run.add_argument('--publish-content', action='store_true', help='Publish ordinary content. Notice visibility is controlled separately.')
    run.add_argument('--archive-notices', action='store_true', help='Import historical notices directly into the public archive without publication notifications.')
    run.add_argument('--skip-pages', action='store_true', help='Import documents, notices and calendar entries, excluding content pages, galleries and unrelated images.')
    run.add_argument('--refresh-assets', action='store_true', help='Download known attachments again to detect files replaced at the same source URL.')
    run.add_argument('--page-map')
    run.add_argument('--notice-map')
    run.add_argument('--base-url', default=os.environ.get('OBEC_VEREJNA_URL', ''))
    reviews = commands.add_parser('reviews', help='Export pending changes without crawling or importing')
    reviews.add_argument('--output', required=True)
    reviews.add_argument('--base-url', default=os.environ.get('OBEC_VEREJNA_URL', ''))
    done = commands.add_parser('acknowledge', help='Mark the exact proposal reviewed, without changing live content')
    done.add_argument('--source-key', required=True)
    done.add_argument('--fingerprint', required=True)
    args = parser.parse_args()
    if args.command == 'run' and (args.delay < .1 or args.max_pages < 1 or args.max_assets < 1):
        parser.error('Use delay >= 0.1 and positive resource limits')
    with connect(autocommit=True) as conn:
        if args.command == 'run':
            report = run_sync(conn, args.state, delay=args.delay, max_pages=args.max_pages,
                max_assets=args.max_assets, publish_content=args.publish_content,
                allow_incomplete=args.allow_incomplete, base_url=args.base_url,
                archive_notices=args.archive_notices, skip_pages=args.skip_pages,
                refresh_assets=args.refresh_assets,
                page_map=json.loads(Path(args.page_map).read_text()) if args.page_map else None,
                notice_map=json.loads(Path(args.notice_map).read_text()) if args.notice_map else None)
            summary = {key: report[key] for key in ('status', 'reason', 'pending_reviews',
                'duration_seconds', 'download_stats', 'refresh_assets') if key in report}
            summary.update({key: len(report[key]) for key in ('new', 'unchanged', 'conflicts', 'capture_errors', 'unavailable_resources', 'skipped_document_pages', 'grouped_gallery_pages', 'skipped_content') if key in report})
            print(json.dumps(summary, ensure_ascii=False), flush=True)
        elif args.command == 'reviews':
            print(json.dumps({'pending_reviews': export_reviews(conn, args.output, args.base_url)}))
        else:
            acknowledge(conn, args.source_key, args.fingerprint)
            print('Proposal marked reviewed. Live content unchanged.')


if __name__ == '__main__':
    main()
