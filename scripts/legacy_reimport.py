#!/usr/bin/env python3
"""Explicit local-only library reset and atomic reimport, with a full backup first."""
import argparse
import json
import os
from pathlib import Path
import subprocess

from legacy import now
from legacy_import import import_bundle, load_bundle
from legacy_scope import require_local_preview
from postgres import connect, database_url, run_tool


def reset_library(conn):
    require_local_preview()
    conn.execute("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))")
    counts = {table: conn.execute('SELECT count(*) FROM ' + table).fetchone()[0]
              for table in ('documents', 'notices', 'attachments', 'notice_events')}
    # Existing pages and calendar descriptions already link to these file IDs.
    attachment_ids = dict(conn.execute('SELECT source_key,attachment_id FROM legacy_sources WHERE attachment_id IS NOT NULL'))
    protected = {
        'legacy_notice_imports': ['legacy_notice_imports_no_delete'],
        'legacy_sources': ['legacy_sources_no_delete'],
        'attachments': ['attachments_preserve_metadata'],
        'notice_events': ['notice_events_no_delete'],
        'notices': ['notices_preserve_record'],
    }
    # DDL and deletion run in the caller's transaction. Rollback restores data and
    # triggers together. Production commands never use this local rehearsal path.
    for table, triggers in protected.items():
        for trigger in triggers:
            conn.execute(f'ALTER TABLE {table} DISABLE TRIGGER {trigger}')
    conn.execute('DELETE FROM legacy_notice_imports')
    conn.execute('DELETE FROM legacy_sources WHERE notice_id IS NOT NULL OR document_id IS NOT NULL')
    conn.execute('DELETE FROM attachments')
    conn.execute('DELETE FROM notice_events')
    conn.execute('DELETE FROM notices')
    conn.execute('DELETE FROM documents')
    for table, triggers in protected.items():
        for trigger in triggers:
            conn.execute(f'ALTER TABLE {table} ENABLE TRIGGER {trigger}')
    conn.execute("INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id) VALUES (%s,'local_library_reset','migration',0)", (now(),))
    return counts, attachment_ids


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True)
    parser.add_argument('--output', required=True, help='New directory for the full backup and import report')
    parser.add_argument('--notice-map', required=True)
    parser.add_argument('--allow-incomplete', action='store_true')
    args = parser.parse_args()
    require_local_preview()
    # Validate every captured file before even attempting a reset.
    load_bundle(args.bundle, args.allow_incomplete)
    output = Path(args.output)
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    backup = output / 'before-library-reset.dump'
    run_tool('pg_dump', database_url(), '--format=custom', '--file', str(backup))
    os.chmod(backup, 0o600)
    subprocess.run(['pg_restore', '--list', str(backup)], check=True, stdout=subprocess.DEVNULL)
    with connect() as conn:
        counts, attachment_ids = reset_library(conn)
        report = import_bundle(conn, args.bundle, publish_content=True,
                               allow_incomplete=args.allow_incomplete, classify_navigation=True,
                               preview_notices=True, notice_map=json.loads(Path(args.notice_map).read_text()),
                               attachment_ids=attachment_ids)
        report['reset_counts'] = counts
        report['backup'] = str(backup)
    (output / 'import.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps({key: report[key] for key in ('classification', 'reset_counts', 'backup')}, indent=2))


if __name__ == '__main__':
    main()
