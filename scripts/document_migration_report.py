#!/usr/bin/env python3
"""Read-only, content-free inventory before/after the unified-document migration.

Use with all writers paused. Output contains numeric IDs, state flags and SHA-256
checksums, never document titles, source URLs, email addresses or file bytes.
"""
import argparse
from collections import Counter
from datetime import date
import json
from pathlib import Path

from psycopg.rows import dict_row
from postgres import connect


def rows(conn, query, parameters=()):
    with conn.cursor(row_factory=dict_row) as cursor:
        return list(cursor.execute(query, parameters))


def hash_sql(value):
    return "encode(sha256(convert_to((" + value + ")::text,'UTF8')),'hex')"


def snapshot(conn, as_of=None):
    """Consistent read-only snapshot. Caller must not have an active write transaction."""
    with conn.transaction():
        conn.execute('SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY')
        canonical = conn.execute("SELECT to_regclass('document_legacy_ids') IS NOT NULL").fetchone()[0]
        as_of = as_of or str(conn.execute("SELECT (now() AT TIME ZONE 'Europe/Prague')::date").fetchone()[0])
        as_of = date.fromisoformat(str(as_of))
        shape = dict(conn.execute("SELECT relname,relkind FROM pg_class WHERE oid IN ('documents'::regclass,'notices'::regclass)"))
        document_digest = hash_sql("jsonb_build_array(title,description,status,created_at,published_at,source_published_on)")
        ordinary = rows(conn, f'SELECT id,{document_digest} AS metadata_sha256 FROM documents ORDER BY id')
        notice_digest = hash_sql("jsonb_build_array(title,coalesce(description,''),status,reference_number,issuer,category_id,published_on,withdraw_on,withdrawn_at,retain_attachments,created_at,published_at,review_json)")
        board = rows(conn, f'SELECT id,{notice_digest} AS metadata_sha256 FROM notices ORDER BY id')
        if canonical:
            visibility = rows(conn, '''SELECT id,document_is_public(d,%s) AS public,
                metadata_restricted,document_files_public(d,%s) AS files_public,lock_state
                FROM documents d ORDER BY id''', (as_of,as_of))
            mapping = rows(conn, 'SELECT old_model,old_id,document_id FROM document_legacy_ids ORDER BY old_model,old_id')
            issues = rows(conn, 'SELECT kind,count(*) AS count FROM document_migration_issues GROUP BY kind ORDER BY kind')
            integrity = rows(conn, "SELECT count(*) FILTER(WHERE lock_state<>'draft' AND content_sha256 IS DISTINCT FROM document_fingerprint(id)) AS invalid_locked_snapshots,count(*) FILTER(WHERE lock_state='draft' AND EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id)) AS editable_posted_documents FROM documents d")[0]
        else:
            visibility = [dict(row,model='document') for row in rows(conn,"SELECT id,status='published' AS public,FALSE AS metadata_restricted,status='published' AS files_public FROM documents ORDER BY id")]
            # Match the old effective projection, including overdue worker rows.
            active = "status='published' AND (published_on IS NULL OR published_on<=%s) AND (withdraw_on IS NULL OR withdraw_on>%s)"
            public = "status IN ('published','withdrawn','archived') AND (status='archived' OR published_on IS NULL OR published_on<=%s)"
            retained = "status IN ('published','withdrawn','archived') AND retain_attachments AND nullif(trim(review_json::jsonb->>'archive_basis'),'') IS NOT NULL AND (review_json::jsonb->>'archive_until')::date>%s"
            visibility += [dict(row,model='notice') for row in rows(conn,
                f'SELECT id,({public}) AS public,NOT ({active}) AS metadata_restricted,coalesce((({public}) AND (({active}) OR ({retained}))),FALSE) AS files_public FROM notices ORDER BY id',
                (as_of,as_of,as_of,as_of,as_of,as_of,as_of))]
            mapping, issues, integrity = [], [], {}
        # Legacy files inherit their parent's visibility. Per-file restriction
        # is introduced by 0029 and starts TRUE without widening parent access.
        attachment_public = 'is_public' if canonical else 'TRUE'
        attachments = rows(conn, f'''SELECT id,document_id,notice_id,size_bytes,octet_length(data) AS byte_length,
            encode(sha256(data),'hex') AS bytes_sha256,sha256 AS recorded_sha256,
            {hash_sql("jsonb_build_array(name,content_type,size_bytes,sort_order,removed_at," + attachment_public + ")")} AS metadata_sha256
            FROM attachments ORDER BY id''')
        linked = {}
        for table, identity in [('legacy_sources','source_key'),('legacy_notice_imports','source_key'),
                                ('notice_events','id'),('publication_outbox','id'),('mail_publication_items','id')]:
            columns = {row[0] for row in conn.execute('SELECT column_name FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=%s', (table,))}
            key = hash_sql(identity) if identity == 'source_key' else identity
            document = 'document_id' if 'document_id' in columns else 'NULL::bigint'
            notice = 'notice_id' if 'notice_id' in columns else 'NULL::bigint'
            # Destination changes are checked independently, without writing URLs.
            payload = "to_jsonb(t)-'document_id'-'notice_id'-'destination'-'source_id'"
            route = (",CASE WHEN destination LIKE '/uredni-deska/%%' THEN 'notice' WHEN destination LIKE '/dokumenty/%%' THEN 'document' ELSE 'other' END AS route,"
                     "CASE WHEN destination ~ '^/(uredni-deska|dokumenty)/[0-9]+$' THEN split_part(destination,'/',3)::bigint END AS route_id,"
                     + hash_sql('destination') + ' AS destination_sha256') if table == 'legacy_sources' else ''
            linked[table] = rows(conn, f'SELECT {key} AS key,{document} AS document_id,{notice} AS notice_id,{hash_sql(payload)} AS row_sha256{route} FROM {table} t ORDER BY {identity}')
        unchanged = {}
        for table in ('mail_queue','subscribers','subscription_consents'):
            unchanged[table] = rows(conn, f'SELECT id,{hash_sql("to_jsonb(t)")} AS row_sha256 FROM {table} t ORDER BY id')
        return dict(version=1,canonical=canonical,as_of_date=str(as_of),shape=shape,
            documents=ordinary,notices=board,visibility=visibility,attachments=attachments,
            mapping=mapping,linked=linked,unchanged=unchanged,migration_issues=issues,integrity=integrity)


def compare(before, after):
    errors, totals = [], Counter()
    def fail(kind, identifier=None):
        totals[kind] += 1
        if len(errors) < 100:
            errors.append(dict(kind=kind,identifier=identifier))
    if before.get('version') != 1 or after.get('version') != 1:
        fail('unsupported_snapshot_version')
    if before['canonical'] or not after['canonical'] or after['shape'] != {'documents':'r','notices':'v'}:
        fail('expected_physical_schema_merge')
    mapping = {(row['old_model'],row['old_id']):row['document_id'] for row in after['mapping']}
    expected = {('document',row['id']) for row in before['documents']} | {('notice',row['id']) for row in before['notices']}
    if set(mapping) != expected or len(set(mapping.values())) != len(expected):
        fail('incomplete_or_colliding_id_mapping')
    if len(after['documents']) != len(expected):
        fail('document_count')
    if len(before['notices']) != len(after['notices']):
        fail('board_count')
    for model, section in [('document','documents'),('notice','notices')]:
        index = {row['id']:row['metadata_sha256'] for row in after[section]}
        for row in before[section]:
            target = mapping.get((model,row['id']))
            if row['metadata_sha256'] != index.get(target):
                fail('content_metadata_changed', f'{model}:{row["id"]}')
            if model == 'document' and target != row['id']:
                fail('ordinary_document_id_changed',row['id'])
    index = {row['id']:row for row in after['visibility']}
    for row in before['visibility']:
        target = mapping.get((row['model'],row['id']))
        current = index.get(target,{})
        if any(row[field] != current.get(field) for field in ('public','metadata_restricted','files_public')):
            fail('effective_public_access_changed', f'{row["model"]}:{row["id"]}')
    files = {row['id']:row for row in after['attachments']}
    if len(files) != len(before['attachments']):
        fail('attachment_count')
    for old in before['attachments']:
        current = files.get(old['id'],{})
        owner = ('notice',old['notice_id']) if old['notice_id'] is not None else ('document',old['document_id'])
        if current.get('document_id') != mapping.get(owner):
            fail('attachment_parent_changed',old['id'])
        if any(old[field] != current.get(field) for field in ('bytes_sha256','byte_length','metadata_sha256')):
            fail('attachment_content_changed',old['id'])
        if current.get('bytes_sha256') and current.get('recorded_sha256') != current.get('bytes_sha256'):
            fail('attachment_hash_mismatch',old['id'])
    for table, old_rows in before['linked'].items():
        current = {row['key']:row for row in after['linked'][table]}
        if len(current) != len(old_rows):
            fail('linked_row_count_changed',table)
        for old in old_rows:
            row = current.get(old['key'],{})
            if old['row_sha256'] != row.get('row_sha256'):
                fail('linked_row_changed',f'{table}:{old["key"]}')
            if old['notice_id'] is not None:
                target = mapping.get(('notice',old['notice_id']))
                if row.get('notice_id') != target:
                    fail('notice_reference_changed',f'{table}:{old["key"]}')
                if table == 'legacy_sources' and row.get('document_id') != target:
                    fail('source_canonical_parent',old['key'])
            elif old['document_id'] is not None and row.get('document_id') != mapping.get(('document',old['document_id'])):
                fail('document_reference_changed',f'{table}:{old["key"]}')
            if table == 'legacy_sources':
                if old.get('route') == 'notice':
                    if row.get('route') != 'document' or row.get('route_id') != mapping.get(('notice',old['route_id'])):
                        fail('source_destination_not_canonical',old['key'])
                elif old['destination_sha256'] != row.get('destination_sha256'):
                    fail('source_destination_changed',old['key'])
    for table, old_rows in before['unchanged'].items():
        if old_rows != after['unchanged'][table]:
            fail('mail_or_subscriber_rows_changed',table)
    for kind, count in after.get('integrity', {}).items():
        if count:
            fail(kind,count)
    corrected = sum(bool(row['bytes_sha256']) and row['recorded_sha256'] != row['bytes_sha256'] for row in before['attachments'])
    return dict(valid=not totals,as_of_date=before['as_of_date'],
        counts=dict(before_documents=len(before['documents']),before_notices=len(before['notices']),
            after_canonical_documents=len(after['documents']),attachments=len(after['attachments']),
            source_mappings=len(after['linked']['legacy_sources']),mail_queue=len(after['unchanged']['mail_queue']),
            publication_outbox=len(after['linked']['publication_outbox'])),
        integrity=after.get('integrity', {}),corrected_recorded_attachment_hashes=corrected,migration_issues=after['migration_issues'],
        discrepancy_counts=dict(totals),discrepancies=errors,discrepancies_truncated=sum(totals.values())>len(errors),
        note='Checks storage and mappings only. It does not establish historical byte authenticity or lawful physical posting.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['snapshot','verify'])
    parser.add_argument('--before', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    arguments = parser.parse_args()
    if arguments.command == 'verify' and arguments.before is None:
        parser.error('verify requires --before')
    before = json.loads(arguments.before.read_text()) if arguments.before else None
    with connect() as connection:
        current = snapshot(connection, before['as_of_date'] if before else None)
    report = compare(before,current) if arguments.command == 'verify' else current
    arguments.output.write_text(json.dumps(report,ensure_ascii=False,indent=2) + '\n')
    print(json.dumps(dict(command=arguments.command,output=str(arguments.output),valid=report.get('valid'))))
    if arguments.command == 'verify' and not report['valid']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
