#!/usr/bin/env python3
"""Move untouched imported notice drafts into the public historical archive."""
import argparse
from collections import Counter
import json
from pathlib import Path

from psycopg.rows import dict_row

from legacy import now
from legacy_notifications import queue_publication
from legacy_scope import archive_review
from postgres import connect


def plan_archive(conn):
    """Read only. Provenance and local edits determine eligibility, never source age."""
    with conn.cursor(row_factory=dict_row) as cursor:
        rows = cursor.execute('''SELECT n.id,n.title,n.status,n.review_json,
            n.published_at IS NOT NULL OR n.withdrawn_at IS NOT NULL AS publication_history,
            n.updated_at IS DISTINCT FROM n.created_at AS changed_timestamp,
            EXISTS(SELECT 1 FROM notice_events e WHERE e.notice_id=n.id) AS has_events,
            EXISTS(SELECT 1 FROM audit_log l WHERE l.entity_type='notice'
                AND l.entity_id=n.id AND l.operation='legacy_imported') AS import_audit,
            EXISTS(SELECT 1 FROM audit_log l WHERE l.entity_type='notice'
                AND l.entity_id=n.id AND l.operation<>'legacy_imported') AS notice_edits,
            EXISTS(SELECT 1 FROM audit_log l JOIN attachments a ON a.id=l.entity_id
                WHERE l.entity_type='attachment' AND a.notice_id=n.id
                AND l.operation<>'legacy_imported') AS attachment_edits,
            (SELECT count(*) FROM attachments a WHERE a.notice_id=n.id) AS attachment_count,
            EXISTS(SELECT 1 FROM attachments a WHERE a.notice_id=n.id AND
                (a.data IS NULL OR a.removed_at IS NOT NULL OR a.sha256 IS NULL
                 OR octet_length(a.data)<>a.size_bytes
                 OR encode(sha256(a.data),'hex')<>a.sha256)) AS unavailable_attachment,
            ARRAY(SELECT s.source_key FROM legacy_sources s WHERE s.notice_id=n.id
                ORDER BY s.source_key) AS source_keys
            FROM notices n WHERE EXISTS(SELECT 1 FROM legacy_sources s WHERE s.notice_id=n.id)
            ORDER BY n.id''').fetchall()
    candidates, excluded = [], []
    for row in rows:
        reasons = []
        if row['status'] != 'draft':
            reasons.append('status_' + row['status'])
        for field in ('publication_history', 'changed_timestamp', 'has_events',
                      'notice_edits', 'attachment_edits', 'unavailable_attachment'):
            if row[field]:
                reasons.append(field)
        if not row['import_audit']:
            reasons.append('missing_import_audit')
        try:
            review = json.loads(row['review_json'])
        except (TypeError, ValueError):
            review = None
        if not isinstance(review, dict):
            reasons.append('invalid_review')
        item = dict(id=row['id'], title=row['title'], source_keys=row['source_keys'],
                    attachment_count=row['attachment_count'])
        if reasons:
            excluded.append(dict(item, reasons=reasons))
        else:
            candidates.append(item)
    return dict(candidate_count=len(candidates), candidate_ids=[row['id'] for row in candidates],
                candidates=candidates, excluded_count=len(excluded), excluded=excluded,
                exclusion_counts=dict(Counter(reason for row in excluded for reason in row['reasons'])),
                archived_count=0, archived_ids=[])


def archive_imported_notices(conn, expected_count):
    """Caller owns the transaction. First public availability notifies subscribers."""
    if type(expected_count) is not int or expected_count < 0:
        raise ValueError('Expected count must be a non-negative integer')
    conn.execute("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))")
    report = plan_archive(conn)
    if report['candidate_count'] != expected_count:
        raise ValueError(f"Expected {expected_count} eligible notice drafts, found {report['candidate_count']}. Run plan again.")
    timestamp = now()
    for item in report['candidates']:
        notice_id = item['id']
        review = json.loads(conn.execute('SELECT review_json FROM notices WHERE id=%s', (notice_id,)).fetchone()[0])
        review.update(archive_review(item['title']))
        conn.execute('''UPDATE notices SET status='archived',retain_attachments=TRUE,
            review_json=%s,updated_at=%s WHERE id=%s''',
            (json.dumps(review, ensure_ascii=False), timestamp, notice_id))
        conn.execute('''INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id)
            VALUES (%s,'legacy_archived','notice',%s)''', (timestamp, notice_id))
        queue_publication(conn, timestamp, notice_id=notice_id)
    report.update(archived_count=expected_count, archived_ids=report['candidate_ids'].copy(), archived_at=timestamp)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['plan', 'apply'])
    parser.add_argument('--report', required=True, help='JSON report with candidate IDs and exclusion reasons')
    parser.add_argument('--expected-count', type=int, help='Required for apply, must match the reviewed plan')
    args = parser.parse_args()
    if args.command == 'apply' and (args.expected_count is None or args.expected_count < 0):
        parser.error('apply requires a non-negative --expected-count from a reviewed plan')
    with connect() as conn:
        if args.command == 'plan':
            conn.execute('SET TRANSACTION READ ONLY')
            report = plan_archive(conn)
        else:
            report = archive_imported_notices(conn, args.expected_count)
    Path(args.report).write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ('candidate_count', 'excluded_count', 'archived_count')}))


if __name__ == '__main__':
    main()
