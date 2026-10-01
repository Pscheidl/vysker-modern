#!/usr/bin/env python3
"""Add source dates and file-level notices without replacing existing imported content."""
import argparse
from datetime import date, datetime, timedelta
import json
from pathlib import Path
import re
from zoneinfo import ZoneInfo

from legacy import dates, now
from legacy_import import load_bundle
from legacy_scope import require_local_preview
from postgres import connect


def reconcile(conn, root, notice_map, *, apply=False, preview=False,
              allow_incomplete=False, as_of=None):
    if preview:
        require_local_preview()
    if conn.execute("SELECT EXISTS(SELECT 1 FROM legacy_notice_imports WHERE metadata ? 'navigation')").fetchone()[0]:
        raise ValueError('Navigation-classified library. Use legacy_import.py with --classify-navigation instead.')
    if apply:
        conn.execute("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))")
    _, items, _ = load_bundle(root, allow_incomplete)
    as_of = as_of or datetime.now(ZoneInfo('Europe/Prague')).date()
    timestamp = now()
    known = {row[0]: row[1:] for row in conn.execute('''SELECT s.source_key,s.document_id,
        s.attachment_id,d.source_published_on,d.title,d.status,a.sha256,a.size_bytes,
        a.data IS NOT NULL AND a.removed_at IS NULL
        FROM legacy_sources s JOIN documents d ON d.id=s.document_id
        JOIN attachments a ON a.id=s.attachment_id''')}
    already = dict(conn.execute('SELECT source_key,notice_id FROM legacy_notice_imports'))
    categories = dict(conn.execute('SELECT name,id FROM categories'))
    report = dict(dates_updated=[], notices_created=[], notices_unchanged=[], review=[],
                  preview=preview, as_of=as_of.isoformat())
    for item in items:
        if 'mime' not in item or item['key'] not in known:
            continue
        key = item['key']
        document_id, attachment_id, stored_date, title, status, sha, size, available = known[key]
        evidence = item.get('evidence', '')
        extracted = dates(evidence.replace('\xa0', ' '))
        published = extracted.get('published_on') or item.get('dates', {}).get('published_on')
        if published:
            if stored_date is not None and stored_date.isoformat() != published:
                raise ValueError('Source date differs from an existing date: ' + key)
            if stored_date is None:
                report['dates_updated'].append({'document_id': document_id, 'published_on': published})
                if apply:
                    conn.execute('UPDATE documents SET source_published_on=%s WHERE id=%s', (published, document_id))
                    conn.execute("INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id) VALUES (%s,'legacy_date_added','document',%s)", (timestamp, document_id))
        category = next((notice_map[p] for p in item.get('parents', []) if p in notice_map), None)
        if not category:
            continue
        if key in already:
            report['notices_unchanged'].append(already[key])
            continue
        if not published or not re.search(r'Vyvěšeno|Vyvěšno|Úřední deska od', evidence, re.I):
            report['review'].append({'key': key, 'reason': 'No complete original posting date. File remains a document.'})
            continue
        if title != item['title'][:300] or (preview and status != 'published'):
            report['review'].append({'key': key, 'reason': 'Local document was edited or is not public. Notice not created.'})
            continue
        if not available or sha != item['capture']['sha256'] or size != item['capture']['size']:
            raise ValueError('Imported attachment differs from the captured file: ' + key)
        posted = date.fromisoformat(published)
        withdrawn = extracted.get('withdraw_on') or item.get('dates', {}).get('withdraw_on')
        end = date.fromisoformat(withdrawn) if withdrawn else None
        # A deadline is not proof of actual withdrawal. Only the isolated local
        # preview uses its following day to demonstrate current/archive filtering.
        preview_deadline = bool(preview and end is None and extracted.get('deadline_on'))
        if preview_deadline:
            end = date.fromisoformat(extracted['deadline_on']) + timedelta(days=1)
        if end and end < posted:
            report['review'].append({'key': key, 'reason': 'End predates posting. Notice not created.'})
            continue
        notice_status = 'draft'
        if preview and posted <= as_of:
            notice_status = 'archived' if end and end <= as_of else 'published'
        review = dict(original_reference=item['url'])
        if preview:
            review.update(archive_title=title,
                          archive_basis='Místní náhled migrace veřejných příloh, vyžaduje samostatné posouzení před produkcí.',
                          archive_until=date.max.isoformat())
        description = 'Převedeno z původního webu.\n\n' + evidence
        if not end:
            description += '\n\nPůvodní web neuvádí úplné datum konce vyvěšení.'
        if preview_deadline:
            description += '\n\nMístní náhled řadí záznam do archivu po uvedené lhůtě. Skutečné datum sejmutí není doloženo.'
        category_id = categories.get('Volby' if re.search(r'vol[eb]', title, re.I) else category)
        if category_id is None:
            raise ValueError('Unknown notice category: ' + category)
        result = dict(document_id=document_id, title=title, published_on=published,
                      withdraw_on=end.isoformat() if end else None, status=notice_status)
        if apply:
            notice_id = conn.execute('''INSERT INTO notices(title,description,category_id,
                published_on,withdraw_on,status,retain_attachments,review_json,created_at,updated_at)
                VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s) RETURNING id''',
                (title,description,category_id,posted,end,notice_status,preview,
                 json.dumps(review,ensure_ascii=False),timestamp,timestamp)).fetchone()[0]
            conn.execute('''INSERT INTO attachments(notice_id,name,content_type,size_bytes,data,sha256)
                SELECT %s,name,content_type,size_bytes,data,sha256 FROM attachments WHERE id=%s''',
                (notice_id,attachment_id))
            metadata = dict(source_dates=extracted, evidence=evidence, parents=item.get('parents', []),
                            source_document_id=document_id, source_attachment_id=attachment_id,
                            preview_end_from_deadline=preview_deadline)
            conn.execute('''INSERT INTO legacy_notice_imports(source_key,notice_id,imported_at,preview_published,metadata)
                VALUES (%s,%s,%s,%s,%s::jsonb)''',
                (key,notice_id,timestamp,preview,json.dumps(metadata,ensure_ascii=False)))
            conn.execute("INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id) VALUES (%s,'legacy_imported','notice',%s)", (timestamp,notice_id))
            result['notice_id'] = notice_id
        report['notices_created'].append(result)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['plan', 'apply'])
    parser.add_argument('--bundle', required=True)
    parser.add_argument('--notice-map', required=True)
    parser.add_argument('--report', required=True)
    parser.add_argument('--allow-incomplete', action='store_true')
    parser.add_argument('--preview-notices', action='store_true', help='Make notices visible only for a local rehearsal, without mail or fabricated publication events.')
    args = parser.parse_args()
    with connect() as conn:
        if args.command == 'plan':
            conn.execute('SET TRANSACTION READ ONLY')
        report = reconcile(conn,args.bundle,json.loads(Path(args.notice_map).read_text()),
                           apply=args.command=='apply',preview=args.preview_notices,
                           allow_incomplete=args.allow_incomplete)
    Path(args.report).write_text(json.dumps(report,ensure_ascii=False,indent=2))
    print(json.dumps({k:len(report[k]) for k in ['dates_updated','notices_created','notices_unchanged','review']},indent=2))


if __name__ == '__main__':
    main()
