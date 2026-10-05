#!/usr/bin/env python3
"""Explicitly replace imported staging content with files, archived notices and events."""
import argparse
import json
import os
from pathlib import Path
import time

from psycopg.rows import dict_row

from legacy_import import import_bundle, load_bundle
from legacy_reimport import reset_imported_content
from postgres import connect


def require_staging():
    if os.environ.get('OBEC_PRODUCTION', '').lower() != 'false':
        raise ValueError('Staging replacement requires explicit OBEC_PRODUCTION=false')


def publication_sources(conn):
    with conn.cursor(row_factory=dict_row) as cursor:
        return cursor.execute('''SELECT s.source_key,
            CASE WHEN s.notice_id IS NOT NULL THEN 'notice' ELSE 'document' END AS kind,
            coalesce(s.notice_id,s.document_id) AS entity_id,
            coalesce(n.status IN ('published','archived','withdrawn'),FALSE)
                OR coalesce(d.status='published',FALSE) AS public,
            coalesce(n.status IN ('published','archived','withdrawn'),FALSE)
                OR coalesce(d.status IN ('published','archived'),FALSE) AS was_public
            FROM legacy_sources s LEFT JOIN notices n ON n.id=s.notice_id
            LEFT JOIN documents d ON d.id=s.document_id
            WHERE s.notice_id IS NOT NULL OR s.document_id IS NOT NULL
            ORDER BY (s.attachment_id IS NOT NULL),s.source_key''').fetchall()


def publication_snapshot(conn, sources):
    """Lock queued mail before replacing the entities its links identify."""
    entities = {f"{row['kind']}:{row['entity_id']}" for row in sources}
    with conn.cursor(row_factory=dict_row) as cursor:
        pending = cursor.execute('''SELECT * FROM publication_outbox
            WHERE CASE WHEN notice_id IS NOT NULL THEN 'notice:'||notice_id
                ELSE 'document:'||document_id END=ANY(%s)''', (list(entities),)).fetchall()
        mails = cursor.execute('''SELECT * FROM mail_queue
            WHERE split_part(deduplication_key,':',1)||':'||
                split_part(deduplication_key,':',2)=ANY(%s)
            ORDER BY (sent_at IS NOT NULL OR cancelled) DESC,id FOR UPDATE''',
            (list(entities),)).fetchall()
    if any(row['locked_until'] > time.time() for row in mails):
        raise ValueError('Imported publication mail is being delivered. Stop the mail worker or retry after delivery.')
    return pending, mails


def restore_publications(conn, sources, pending, mails):
    destinations = {row['source_key']: row for row in publication_sources(conn) if row['public']}
    remap = {}
    for source in sources:
        target = destinations.get(source['source_key'])
        if target:
            remap.setdefault((source['kind'], source['entity_id']), (target['kind'], target['entity_id']))
    for row in mails:
        kind, entity_id, recipient = row['deduplication_key'].split(':')
        target = remap.get((kind, int(entity_id)))
        if not target:
            conn.execute('UPDATE mail_queue SET cancelled=TRUE,body=NULL WHERE id=%s AND sent_at IS NULL', (row['id'],))
            continue
        target_kind, target_id = target
        key = f'{target_kind}:{target_id}:{recipient}'
        duplicate = conn.execute('SELECT EXISTS(SELECT 1 FROM mail_queue WHERE deduplication_key=%s AND id<>%s)',
                                 (key, row['id'])).fetchone()[0]
        if duplicate:
            conn.execute('UPDATE mail_queue SET cancelled=TRUE,body=NULL WHERE id=%s AND sent_at IS NULL', (row['id'],))
            continue
        route = 'uredni-deska' if kind == 'notice' else 'dokumenty'
        target_route = 'uredni-deska' if target_kind == 'notice' else 'dokumenty'
        body = row['body']
        if body is not None:
            body = body.replace(f'/{route}/{entity_id}\n', f'/{target_route}/{target_id}\n')
        conn.execute('UPDATE mail_queue SET deduplication_key=%s,body=%s WHERE id=%s', (key, body, row['id']))
    for row in pending:
        source = ('notice', row['notice_id']) if row['notice_id'] is not None else ('document', row['document_id'])
        target = remap.get(source)
        if target:
            kind, entity_id = target
            conn.execute('''INSERT INTO publication_outbox
                (notice_id,document_id,subscriber_id,consent_id,created_at)
                VALUES (%s,%s,%s,%s,%s) ON CONFLICT DO NOTHING''',
                (entity_id if kind == 'notice' else None, entity_id if kind == 'document' else None,
                 row['subscriber_id'], row['consent_id'], row['created_at']))


def replace_import(conn, root, expected_source_count, *, archive_notices, skip_pages, notice_map=None):
    """Validate first, then replace atomically. A failure restores the previous content."""
    require_staging()
    if not archive_notices or not skip_pages:
        raise ValueError('Staging replacement requires --archive-notices and --skip-pages')
    if type(expected_source_count) is not int or expected_source_count < 0:
        raise ValueError('Expected source count must be a non-negative integer')
    # Every captured object is validated before any old content is deleted.
    # import_bundle validates again inside the transaction in case the bundle changes.
    load_bundle(root)
    with conn.transaction():
        conn.execute("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))")
        source_count = conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0]
        if source_count != expected_source_count:
            raise ValueError(f'Expected {expected_source_count} imported sources, found {source_count}. Review the current import before replacing it.')
        sources = publication_sources(conn)
        pending, mails = publication_snapshot(conn, sources)
        counts, attachment_ids = reset_imported_content(conn, audit_operation='staging_import_reset')
        report = import_bundle(conn, root, publish_content=True, classify_navigation=True,
                               archive_notices=True, skip_pages=True, notice_map=notice_map,
                               attachment_ids=attachment_ids,
                               notification_excluded_source_keys={row['source_key'] for row in sources if row['was_public']})
        restore_publications(conn, sources, pending, mails)
        report.update(reset_counts=counts, reset_scope='all_imported', previous_source_count=source_count,
                      resulting_source_count=conn.execute('SELECT count(*) FROM legacy_sources').fetchone()[0],
                      backup_created=False)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, help='Complete, validated offline capture')
    parser.add_argument('--report', required=True, help='JSON result of the completed replacement')
    parser.add_argument('--expected-source-count', required=True, type=int, help='Reviewed number of existing imported source identities')
    parser.add_argument('--archive-notices', required=True, action='store_true', help='Put imported notices directly into the public archive')
    parser.add_argument('--skip-pages', required=True, action='store_true', help='Import documents, notices and events without content pages or galleries')
    parser.add_argument('--notice-map', help='Optional JSON mapping of source sections to notice categories')
    args = parser.parse_args()
    require_staging()
    notice_map = json.loads(Path(args.notice_map).read_text()) if args.notice_map else None
    with connect() as conn:
        report = replace_import(conn, args.bundle, args.expected_source_count,
                                archive_notices=args.archive_notices, skip_pages=args.skip_pages,
                                notice_map=notice_map)
    Path(args.report).write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ('reset_counts', 'previous_source_count', 'resulting_source_count')}, indent=2))


if __name__ == '__main__':
    main()
