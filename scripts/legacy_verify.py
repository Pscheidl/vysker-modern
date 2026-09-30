#!/usr/bin/env python3
"""Compare an imported preview with its database, including every attachment hash."""
import argparse
import hashlib
import json
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener, urlopen

from postgres import connect


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def verify(base):
    base = base.rstrip('/')
    parsed = urlsplit(base)
    if parsed.scheme not in ('http', 'https') or parsed.path or parsed.query or parsed.fragment:
        raise ValueError('Supply the preview origin without a path')
    failures = []
    redirects = downloads = byte_count = 0
    opener = build_opener(NoRedirect())
    with connect() as conn:
        conn.execute('SET TRANSACTION READ ONLY')
        rows = conn.execute("""SELECT s.source_url,s.destination,
            coalesce(p.published OR d.status='published' OR e.published OR n.status IN ('published','archived','withdrawn'),FALSE)
            FROM legacy_sources s LEFT JOIN pages p ON p.id=s.page_id
            LEFT JOIN documents d ON d.id=s.document_id LEFT JOIN events e ON e.id=s.event_id
            LEFT JOIN notices n ON n.id=s.notice_id ORDER BY s.source_key""").fetchall()
        for source, destination, public in rows:
            url = urlsplit(source)
            if url.path == '/':
                continue
            path = url.path + ('?' + url.query if url.query else '')
            try:
                response = opener.open(Request(base + path, method='HEAD'), timeout=20)
            except HTTPError as error:
                response = error
            with response:
                if public and (response.code != 301 or response.headers.get('Location') != destination):
                    failures.append({'path': path, 'error': 'Redirect mismatch', 'status': response.code})
                elif not public and response.code == 301:
                    failures.append({'path': path, 'error': 'Draft is redirected'})
                else:
                    redirects += 1
        files = conn.execute("""SELECT a.id,a.sha256,a.size_bytes FROM attachments a
            JOIN documents d ON d.id=a.document_id JOIN legacy_sources s ON s.attachment_id=a.id
            WHERE d.status='published' AND a.data IS NOT NULL AND a.removed_at IS NULL ORDER BY a.id""").fetchall()
        for id, expected, size in files:
            checksum = hashlib.sha256()
            actual = 0
            with urlopen(f'{base}/api/v1/attachments/{id}', timeout=30) as response:
                while chunk := response.read(65536):
                    checksum.update(chunk)
                    actual += len(chunk)
            if actual != size or checksum.hexdigest() != expected:
                failures.append({'attachment_id': id, 'error': 'Downloaded bytes differ'})
            else:
                downloads += 1
                byte_count += actual
        mail = conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0]
        counts = {table: conn.execute('SELECT count(*) FROM ' + table).fetchone()[0]
                  for table in ['legacy_sources', 'pages', 'documents', 'events', 'notices', 'attachments']}
    return dict(counts=counts, redirects_checked=redirects, downloads_checked=downloads,
                downloaded_bytes=byte_count, mail_queue=mail, failures=failures)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base-url', required=True)
    parser.add_argument('--report', required=True)
    args = parser.parse_args()
    report = verify(args.base_url)
    Path(args.report).write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    raise SystemExit(1 if report['failures'] else 0)
