"""Canonical document import policy, immutable candidates and explicit review.

No function in this module edits a locked original. Candidate files never have a
public download route. Callers own the transaction and shared writer lock.
"""
import json
from datetime import date, datetime
from zoneinfo import ZoneInfo

from psycopg.rows import dict_row

from legacy import digest, source_key

SOURCE_ID = 'vismo:vysker.cz'


def source_policy(conn):
    row = conn.execute('''SELECT mode,allow_publication,allow_operational
        FROM document_import_sources WHERE id=%s''', (SOURCE_ID,)).fetchone()
    if row is None or row[0] == 'disabled':
        raise ValueError('Legacy source is disabled or has no explicit import policy')
    return dict(mode=row[0], allow_publication=row[1] and row[0] != 'proposals',
                allow_operational=row[2] and row[0] == 'mirror')


def configure_source(conn, mode, allow_publication=False, allow_operational=False, reason='', actor_id=None):
    if mode not in ('one_time', 'mirror', 'proposals', 'disabled') or not reason.strip():
        raise ValueError('An explicit source mode and a nonempty reason are required')
    if allow_operational:
        # This importer is additive. Configuration must never imply implemented
        # authority to rewrite plans or local publication/withdrawal evidence.
        raise ValueError('Legacy importer does not support operational authority')
    conn.execute("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))")
    old = conn.execute('SELECT mode,allow_publication,allow_operational FROM document_import_sources WHERE id=%s FOR UPDATE',
                       (SOURCE_ID,)).fetchone()
    conn.execute('''UPDATE document_import_sources SET mode=%s,allow_publication=%s,
        allow_operational=FALSE,updated_at=now() WHERE id=%s''', (mode, allow_publication, SOURCE_ID))
    conn.execute('''INSERT INTO audit_log(actor_id,operation,entity_type,details)
        VALUES (%s,'import_source_configured','import_source',%s)''',
        (actor_id, json.dumps(dict(source_id=SOURCE_ID, before=old,
                                  after=[mode, allow_publication, False], reason=reason))))


def observe(conn, items, timestamp, fingerprints):
    # Metadata only, bounded by the configured crawler page/file limits. File
    # bytes are always loaded one at a time by the capture and revision paths.
    keys = [item['key'] for item in items]
    known = set(row[0] for row in conn.execute('SELECT source_key FROM legacy_sources WHERE source_key=ANY(%s)', (keys,)))
    retained = [item for item in items if item['key'] in known]
    for offset in range(0, len(retained), 500):
        batch = retained[offset:offset + 500]
        conn.execute('''INSERT INTO document_source_observations(source_key,last_observed_at,last_fingerprint)
            SELECT key,%s,signature FROM unnest(%s::text[],%s::text[]) AS batch(key,signature)
            ON CONFLICT (source_key) DO UPDATE SET
            last_observed_at=EXCLUDED.last_observed_at,last_fingerprint=EXCLUDED.last_fingerprint''',
            (timestamp, [item['key'] for item in batch], [fingerprints[item['key']] for item in batch]))
        conn.execute('''INSERT INTO document_source_aliases(source_id,source_url,source_key)
            SELECT %s,url,key FROM unnest(%s::text[],%s::text[]) AS batch(url,key)
            ON CONFLICT DO NOTHING''',
            (SOURCE_ID, [item['url'] for item in batch], [item['key'] for item in batch]))


def finalize_board_documents(conn, document_ids, timestamp, allow_publication=False):
    """Lock only after every validated attachment has been inserted.

    Source date precision stays DATE. Neither capture nor import creates a local
    posting timestamp, and the imported bytes are explicitly a current capture.
    """
    if not document_ids:
        return
    with conn.cursor(row_factory=dict_row) as cursor:
        rows = cursor.execute('''SELECT id,published_on,withdraw_on,board_status,review_json,
            retain_attachments,public_access FROM documents WHERE id=ANY(%s)''', (list(document_ids),)).fetchall()
    for row in rows:
        origin = conn.execute('SELECT source_url,metadata,captured_at FROM legacy_sources WHERE document_id=%s ORDER BY attachment_id NULLS FIRST,source_key LIMIT 1',
                              (row['id'],)).fetchone()
        removed = None
        captured = datetime.fromisoformat((origin[2] if origin else timestamp).replace('Z', '+00:00')).astimezone(ZoneInfo('Europe/Prague')).date()
        if origin:
            removal = (origin[1].get('source_dates') or {}).get('removed_on') or (origin[1].get('dates') or {}).get('removed_on')
            if removal:
                candidate = date.fromisoformat(removal)
                if candidate <= captured and (row['published_on'] is None or candidate >= row['published_on']):
                    removed = candidate
        documented = row['published_on'] is not None and row['published_on'] <= captured
        archive = row['board_status'] == 'archived'
        public = allow_publication and documented and row['board_status'] in ('published', 'archived') and (not archive or (documented and removed is not None))
        review = json.loads(row['review_json'])
        public_until = review.get('archive_until') if public and archive else None
        conn.execute('UPDATE documents SET source_published_on=CASE WHEN %s THEN published_on END WHERE id=%s', (documented, row['id']))
        conn.execute('''UPDATE documents SET lock_state=%s,locked_at=%s,
            content_sha256=document_fingerprint(id),public_access=%s,
            status=CASE WHEN %s THEN 'published' ELSE 'draft' END,
            public_until=%s WHERE id=%s''',
            ('locked' if documented else 'review', timestamp, public, public, public_until, row['id']))
        conn.execute('''INSERT INTO document_postings(document_id,origin,certainty,started_on,
            planned_removal_on,removed_on,after_action,public_until,snapshot_sha256,source_url)
            VALUES (%s,'source',%s,%s,%s,%s,%s,%s,document_fingerprint(%s),%s)''',
            (row['id'], 'documented' if documented else 'uncertain', row['published_on'],
             row['withdraw_on'], removed, 'retain' if row['retain_attachments'] else 'hide',
             public_until, row['id'], origin[0] if origin else None))
        conn.execute('''INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id,details)
            VALUES (%s,'source_snapshot_captured','document',%s,%s)''',
            (timestamp, row['id'], json.dumps(dict(captured_at=timestamp,
                historical_bytes_verified=False, certainty='documented' if documented else 'uncertain'))))


def queue_revision(conn, item, signature, content_fingerprint, capture, timestamp, items):
    owner = conn.execute('''SELECT COALESCE(document_id,notice_id),attachment_id
        FROM legacy_sources WHERE source_key=%s''', (item['key'],)).fetchone()
    if owner is None:
        raise ValueError('Revision has no stable source mapping')
    proposal = dict(item)
    item_index = {entry['key']: entry for entry in items}
    files = []
    snapshot_item = item
    if owner[0] is not None:
        # Always capture the complete currently observed source document. This
        # prevents acceptance of a later changed file from bringing back an old
        # sibling file from the initial import or from losing a removed file.
        roots = conn.execute('''SELECT source_key FROM legacy_sources
            WHERE document_id=%s AND attachment_id IS NULL ORDER BY source_key''', (owner[0],)).fetchall()
        snapshot_item = next((item_index[key] for key, in roots
                              if key in item_index and 'mime' not in item_index[key]), item)
    if 'mime' in snapshot_item:
        files = [snapshot_item]
        proposal['replacement_attachment_id'] = owner[1]
        proposal['replaces_attachment_list'] = True
    elif owner[0] is not None:
        for reference in snapshot_item.get('assets', []):
            entry = item_index.get(source_key(reference['url']))
            if entry is None or 'mime' not in entry:
                proposal['incomplete'] = True
            else:
                files.append(entry)
        proposal['replaces_attachment_list'] = True
        proposal['observed_item_title'] = proposal.get('title')
        proposal['title'] = snapshot_item['title']
        proposal['content'] = snapshot_item.get('content', '')
    proposal['change_origin'] = 'observed_source_change'
    result = conn.execute('''INSERT INTO document_import_revisions
        (source_key,document_id,fingerprint,content_fingerprint,proposal,source_data,first_seen_at)
        VALUES (%s,%s,%s,%s,%s::jsonb,%s,%s)
        ON CONFLICT (source_key,fingerprint) DO NOTHING RETURNING id''',
        (item['key'], owner[0], signature, content_fingerprint, json.dumps(proposal, ensure_ascii=False),
         b'', timestamp)).fetchone()
    if result is None:
        return
    revision_id = result[0]
    file_keys = [file['key'] for file in files]
    visibility = dict(conn.execute('''SELECT s.source_key,a.is_public FROM legacy_sources s
        JOIN attachments a ON a.id=s.attachment_id WHERE s.source_key=ANY(%s)''', (file_keys,)))
    for position, file in enumerate(files):
        raw = capture.blob(file['capture']['sha256'])
        conn.execute('''INSERT INTO document_import_revision_files
            (revision_id,position,name,content_type,size_bytes,sha256,data,is_public) VALUES (%s,%s,%s,%s,%s,%s,%s,%s)''',
            (revision_id, position, file['name'], file['mime'], len(raw), digest(raw), raw, visibility.get(file['key'], True)))
    conn.execute('''INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id,details)
        VALUES (%s,'source_revision_observed','document',%s,%s)''',
        (timestamp, owner[0], json.dumps(dict(revision_id=revision_id, source_key=item['key'], fingerprint=signature))))


def decide_revision(conn, revision_id, decision, reason, actor_id=None):
    if decision not in ('accept', 'reject') or not reason.strip():
        raise ValueError('Decision and nonempty internal reason are required')
    new_id = conn.execute('SELECT document_decide_revision(%s,%s,%s,%s)',
                          (revision_id, decision, reason, actor_id)).fetchone()[0]
    return dict(revision_id=revision_id, state='accepted' if decision == 'accept' else 'rejected',
                document_id=new_id)


def refuse_destructive_reimport(conn):
    if conn.execute("SELECT to_regclass('document_postings') IS NOT NULL").fetchone()[0]:
        raise ValueError('Destructive reimport is disabled for the canonical document library. '
                         'Use additive sync and immutable revision review, or restore a verified full database backup.')
