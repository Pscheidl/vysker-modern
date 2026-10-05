#!/usr/bin/env python3
"""Explicitly replace imported staging content with files, archived notices and events."""
import argparse
import json
import os
from pathlib import Path

from legacy_import import import_bundle, load_bundle
from legacy_reimport import reset_imported_content
from postgres import connect


def require_staging():
    if os.environ.get('OBEC_PRODUCTION', '').lower() != 'false':
        raise ValueError('Staging replacement requires explicit OBEC_PRODUCTION=false')


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
        counts, attachment_ids = reset_imported_content(conn, audit_operation='staging_import_reset')
        report = import_bundle(conn, root, publish_content=True, classify_navigation=True,
                               archive_notices=True, skip_pages=True, notice_map=notice_map,
                               attachment_ids=attachment_ids)
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
