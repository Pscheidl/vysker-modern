#!/usr/bin/env python3
"""Synthetic, disposable PostgreSQL benchmark for the unified document migration.

Never opens the application's configured database. TEST_DATABASE_URL must point to
an isolated test server, and every workload creates and drops its own database.
The SQL workload deliberately excludes HTTP rendering and network download time.
"""
import argparse
from datetime import date
import hashlib
import importlib
import json
from pathlib import Path
import resource
import statistics
import sys
import tempfile
import time
import tracemalloc

import psycopg
from psycopg import sql

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from postgres import temporary_database


class CountedCursor(psycopg.Cursor):
    def execute(self, *args, **kwargs):
        self.connection.statements += 1
        return super().execute(*args, **kwargs)


class CountedConnection(psycopg.Connection):
    statements = 0


def migrate(conn, source, before=None, after=None):
    for path in sorted((source / 'migrations').glob('*.sql')):
        number = int(path.name.split('_', 1)[0])
        if (before is not None and number >= before) or (after is not None and number < after):
            continue
        conn.execute(path.read_text())
    conn.commit()


def seed(conn, size, with_provenance=False):
    """Half ordinary documents, half board records, with colliding legacy IDs."""
    half = size // 2
    conn.execute('''INSERT INTO documents(title,description,status,created_at,published_at,source_published_on)
        SELECT 'Rozpočet anonymní '||i,'Veřejný dokument pro měření '||i,
            CASE WHEN i %% 10=0 THEN 'draft' ELSE 'published' END,
            '2026-10-01T08:00:00Z','2026-10-01T08:00:00Z',
            CASE WHEN i %% 11=0 THEN NULL ELSE DATE '2020-01-01' + (i %% 2000)::int END
        FROM generate_series(1,%s) AS i''', (half,))
    conn.execute('''INSERT INTO notices(title,description,reference_number,category_id,published_on,
        published_at,withdraw_on,withdrawn_at,status,retain_attachments,review_json)
        SELECT 'Rozpočet oznámení '||i,'Veřejná písemnost pro měření '||i,'ANON/'||i,
            (i %% 7) + 1,DATE '2020-01-01' + (i %% 2000)::int,
            '2020-01-01T08:00:00Z',CASE WHEN i %% 10=1 THEN NULL ELSE DATE '2025-10-01' END,
            CASE WHEN i %% 10=1 THEN NULL ELSE '2025-10-01T08:00:00Z' END,
            CASE WHEN i %% 10=0 THEN 'draft' WHEN i %% 10=1 THEN 'published' ELSE 'archived' END,
            i %% 5<>0,
            jsonb_build_object('archive_title','Rozpočet oznámení '||i,
                'archive_basis','Synthetic fixture','archive_until','2099-12-31')::text
        FROM generate_series(1,%s) AS i''', (half,))
    # Small byte payloads ensure measurements cover metadata query behavior, not disk capacity.
    for owner in ('notice', 'document'):
        conn.execute(sql.SQL('''INSERT INTO attachments({parent},name,content_type,size_bytes,sort_order,data,sha256)
            SELECT i,'priloha-'||a||'.pdf','application/pdf',32,a,
                decode(repeat('25',32),'hex'),encode(sha256(decode(repeat('25',32),'hex')),'hex')
            FROM generate_series(1,%s) i CROSS JOIN generate_series(1,2) a''')
            .format(parent=sql.Identifier(owner + '_id')), (half,))
    conn.execute("INSERT INTO pages(slug,title,content,published,updated_at) SELECT 'mereni-'||i,'Obecní stránka '||i,'Rozpočet obce pro veřejnost',TRUE,'2020-01-01T08:00:00Z' FROM generate_series(1,%s) i", (max(20, size // 100),))
    if with_provenance:
        for owner in ('notice', 'document'):
            conn.execute(sql.SQL('''INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,{parent},destination)
                SELECT %s||i,%s||i,repeat('a',64),'2026-10-01T08:00:00Z','2026-10-01T08:00:00Z','{{}}'::jsonb,i,%s||i
                FROM generate_series(1,%s) i''').format(parent=sql.Identifier(owner + '_id')),
                (owner + '/', 'https://www.vysker.cz/?kind=' + owner + '&id=', '/uredni-deska/' if owner == 'notice' else '/dokumenty/', half))
    conn.commit()
    conn.execute('ANALYZE')
    conn.commit()


def folded(expr):
    mapping = str.maketrans('ÁČĎÉĚÍŇÓŘŠŤÚŮÝŽáčďéěíňóřšťúůýž', 'acdeeinorstuuyzacdeeinorstuuyz')
    for char in mapping:
        expr = f"replace({expr},'{chr(char)}','{chr(char).translate(mapping)}')"
    return f'lower({expr})'


def baseline_queries():
    # Same selection, predicates and order as backend/documents.rs and search.rs.
    public = '''WITH notice_state AS (SELECT n.*, c.name AS category,
        status='published' AND (published_on IS NULL OR published_on<=DATE '2026-11-01')
        AND (withdraw_on IS NULL OR withdraw_on>DATE '2026-11-01') AS active
        FROM notices n LEFT JOIN categories c ON c.id=n.category_id
        WHERE status IN ('published','archived','withdrawn')
        AND (status='archived' OR published_on IS NULL OR published_on<=DATE '2026-11-01')),
        public AS (SELECT id,'notice' AS kind,
        CASE WHEN active THEN title ELSE coalesce(nullif(trim(review_json::jsonb->>'archive_title'),''),'Záznam úřední desky #'||id) END AS title,
        CASE WHEN active THEN coalesce(description,'') ELSE '' END AS description,
        CASE WHEN active THEN coalesce(reference_number,'')||' '||coalesce(issuer,'') ELSE '' END AS extra,
        '/uredni-deska/'||id AS path,coalesce(category,'Ostatní') AS category,
        coalesce(published_on::text,'') AS posted,NOT active AS archived FROM notice_state
        UNION ALL SELECT id,'document',title,coalesce(description,''),'','/dokumenty/'||id,'',
        coalesce(source_published_on::text,published_at,''),FALSE FROM documents WHERE status='published'
        UNION ALL SELECT id,'page',title,content,'','/stranky/'||slug,'',updated_at,FALSE FROM pages WHERE published=TRUE)
        SELECT {selection} FROM public WHERE {where}'''
    selected = 'id,kind,title,substr(description,1,400) AS description,path,category,posted,archived'
    order = " ORDER BY (posted='') ASC,posted DESC,kind,id DESC LIMIT 20 OFFSET 0"
    def search(where):
        return [public.format(selection='count(*)', where=where), public.format(selection=selected, where=where) + order]
    word = "strpos(" + folded("title||' '||description||' '||extra||' '||category") + ",'rozpocet')>0"
    return {
        'catalog': search("kind='document' AND archived=FALSE"),
        'current_board': search("kind='notice' AND archived=FALSE"),
        'public_archive': search("kind='notice' AND archived=TRUE"),
        'global_search': search(word),
        'selective_search': search("strpos(" + folded("title||' '||description||' '||extra||' '||category") + ",'anonymni')>0 AND strpos(" + folded("title||' '||description||' '||extra||' '||category") + ",'9999')>0"),
        'document_detail': ["SELECT title,description FROM documents WHERE id=1 AND status='published'",
            "SELECT id,name,size_bytes,data IS NOT NULL AND removed_at IS NULL FROM attachments WHERE document_id=1 ORDER BY sort_order,id",
            "SELECT s.source_url FROM legacy_sources s LEFT JOIN legacy_notice_imports n ON n.source_key=s.source_key WHERE s.notice_id=NULL OR n.notice_id=NULL OR s.document_id=1 ORDER BY (s.attachment_id IS NOT NULL),s.source_key LIMIT 1"],
    }


def unified_queries():
    def search(scope, word=None):
        stamp = 'd.published_on' if scope == 'current_board' else 'd.catalog_date'
        archived = "((d.board_status IS NULL AND d.status='archived') OR d.has_removed_posting) AND NOT d.has_current_posting AND d.board_status IS DISTINCT FROM 'scheduled'"
        prefix = f"""WITH public AS (SELECT d.id,
            CASE WHEN (d.board_status='published' AND d.has_current_posting) OR (d.has_removed_posting AND NOT d.has_current_posting AND d.board_status IS DISTINCT FROM 'scheduled') THEN 'notice' ELSE 'document' END AS kind,
            CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Záznam úřední desky #'||d.id) ELSE d.title END AS title,CASE WHEN d.metadata_restricted THEN '' ELSE substr(d.description,1,400) END AS description,'/dokumenty/'||d.id AS path,
            coalesce(c.name,'Ostatní') AS category,coalesce({stamp}::text,'') AS posted,
            {stamp} AS sort_date,({archived}) AS archived
            FROM documents d LEFT JOIN categories c ON c.id=d.category_id
            WHERE document_is_public(d,DATE '2026-11-01')"""
        if scope == 'current_board':
            prefix += " AND d.board_status='published' AND d.has_current_posting"
        elif scope == 'public_archive':
            prefix += f" AND ({archived})"
        for term in (word or '').split():
            prefix += f" AND d.search_text LIKE '%{term}%'"
        if scope in ('global_search', 'selective_search'):
            prefix += " UNION ALL SELECT id,'page',title,substr(content,1,400),'/stranky/'||slug,'',updated_at,updated_at::date,FALSE FROM pages WHERE published=TRUE"
            for term in (word or '').split():
                prefix += f" AND document_fold(title||' '||content) LIKE '%{term}%'"
        prefix += ') SELECT '
        return [prefix + 'count(*) FROM public', prefix + 'id,kind,title,substr(description,1,400) AS description,path,category,posted,archived FROM public ORDER BY sort_date DESC NULLS LAST,id DESC,kind LIMIT 20 OFFSET 0']
    return {
        'catalog': search('catalog'),
        'current_board': search('current_board'),
        'public_archive': search('public_archive'),
        'global_search': search('global_search', 'rozpocet'),
        'selective_search': search('selective_search', 'anonymni 9999'),
        'document_detail': ["SELECT CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Dokument #'||d.id) ELSE d.title END AS title,CASE WHEN d.metadata_restricted THEN '' ELSE coalesce(d.description,'') END AS description,CASE WHEN d.metadata_restricted THEN NULL ELSE d.reference_number END AS reference_number,CASE WHEN d.metadata_restricted THEN NULL ELSE d.issuer END AS issuer,d.document_date::text AS document_date,d.source_published_on::text AS source_published_on,c.name AS topic,document_files_public(d,DATE '2026-11-01') AS files_public FROM documents d LEFT JOIN categories c ON c.id=d.category_id WHERE d.id=1 AND document_is_public(d,DATE '2026-11-01')",
            "SELECT id,name,size_bytes FROM attachments WHERE document_id=1 AND is_public AND data IS NOT NULL AND removed_at IS NULL AND EXISTS(SELECT 1 FROM documents d WHERE d.id=1 AND document_files_public(d,DATE '2026-11-01')) ORDER BY sort_order,id",
            "SELECT id,origin,certainty,started_on,started_at,planned_removal_on,removed_on,removed_at FROM document_postings WHERE document_id=1 ORDER BY started_on DESC NULLS LAST,id DESC",
            "SELECT id,(payload->>'posting_id')::bigint,payload->>'field',payload->>'original',payload->>'corrected',occurred_at FROM document_events WHERE document_id=1 AND kind='evidence_corrected' ORDER BY id",
            "SELECT d.id,CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Dokument #'||d.id) ELSE d.title END AS title,r.kind,'following'::text AS direction FROM document_relations r JOIN documents d ON d.id=r.related_id WHERE r.original_id=1 AND document_is_public(d,DATE '2026-11-01') UNION ALL SELECT d.id,CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Dokument #'||d.id) ELSE d.title END AS title,r.kind,'original'::text AS direction FROM document_relations r JOIN documents d ON d.id=r.original_id WHERE r.related_id=1 AND document_is_public(d,DATE '2026-11-01') ORDER BY id",
            "WITH source_keys AS (SELECT source_key FROM legacy_sources WHERE document_id=ANY(ARRAY[NULL::bigint,1::bigint]) UNION SELECT source_key FROM legacy_notice_imports WHERE notice_id=NULL UNION SELECT source_key FROM document_import_revisions WHERE accepted_document_id=ANY(ARRAY[NULL::bigint,1::bigint])) SELECT s.source_url,(s.attachment_id IS NULL OR EXISTS(SELECT 1 FROM attachments a WHERE a.id=s.attachment_id AND a.is_public AND a.data IS NOT NULL AND a.removed_at IS NULL)) AS link_allowed FROM source_keys k JOIN legacy_sources s USING(source_key) ORDER BY (s.attachment_id IS NOT NULL),s.source_key LIMIT 1"],
    }


def measure(conn, queries, repetitions=21):
    # Warm cache first, then measure end-to-end Python/driver/query latency.
    for query in queries:
        conn.execute(query).fetchall()
    samples = []
    tracemalloc.start()
    start_count = conn.statements
    for _ in range(repetitions):
        start = time.perf_counter()
        for query in queries:
            rows = conn.execute(query).fetchall()
        samples.append((time.perf_counter() - start) * 1000)
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    plans = [conn.execute('EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) ' + q).fetchone()[0][0] for q in queries]
    return {'median_ms': round(statistics.median(samples), 3),
            'p95_ms': round(sorted(samples)[int(len(samples) * .95)], 3),
            'statements_per_call': (conn.statements - start_count - len(queries)) // repetitions,
            'peak_python_kib': round(peak / 1024, 1), 'last_page_rows': len(rows),
            'plans': plans, 'sql': queries}


def make_bundle(root, size):
    objects = root / 'objects'
    objects.mkdir()
    raw = b'%PDF-1.7\nSynthetic public document fixture\n%%EOF'
    checksum = hashlib.sha256(raw).hexdigest()
    (objects / checksum).write_bytes(raw)
    assets = [dict(url=f'https://vysker.cz/assets/File.ashx?id_dokumenty={i}&id_org=18774',
        title=f'Dokument {i}', name=f'dokument-{i}.pdf', mime='application/pdf',
        parents=[], dates={'published_on': '2020-03-02'},
        capture={'sha256': checksum, 'size': len(raw), 'captured_at': '2026-10-01T12:00:00+00:00'})
        for i in range(1, size + 1)]
    manifest = dict(version=1, origin='https://vysker.cz', pages=[], assets=assets, aliases={}, errors=[], complete=True)
    (root / 'manifest.json').write_text(json.dumps(manifest))
    return manifest


def run_import(source, conn, size):
    # Run the actual source-version importer in this process for SQL/memory accounting.
    sys.path.insert(0, str(source / 'scripts'))
    for name in list(sys.modules):
        if name.startswith('legacy'):
            del sys.modules[name]
    importer = importlib.import_module('legacy_import')
    measurements = {}
    with tempfile.TemporaryDirectory(prefix='document-benchmark-bundle-') as directory:
        root = Path(directory)
        manifest = make_bundle(root, size)
        for mode in ('initial', 'unchanged', 'small_change', 'same_change'):
            if mode == 'small_change':
                raw = b'%PDF-1.7\nChanged synthetic public document fixture\n%%EOF'
                sha = hashlib.sha256(raw).hexdigest()
                (root / 'objects' / sha).write_bytes(raw)
                manifest['assets'][0]['capture'].update(sha256=sha, size=len(raw))
                (root / 'manifest.json').write_text(json.dumps(manifest))
            count = conn.statements
            tracemalloc.start()
            start = time.perf_counter()
            result = importer.import_bundle(conn, root, publish_content=True, sync=True)
            conn.commit()
            elapsed = time.perf_counter() - start
            _, peak = tracemalloc.get_traced_memory()
            tracemalloc.stop()
            measurements[mode] = {'elapsed_ms': round(elapsed * 1000, 2),
                'sql_statements': conn.statements - count,
                'peak_python_mib': round(peak / 1024 / 1024, 2),
                'new': len(result['new']), 'unchanged': len(result['unchanged']),
                'conflicts': len(result['conflicts']),
                'pending_reviews': result.get('pending_reviews', 0)}
    return measurements


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, default=ROOT)
    p.add_argument('--phase', choices=['baseline', 'unified'], required=True)
    p.add_argument('--sizes', type=int, nargs='+', default=[2000, 20000])
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--queries-only', action='store_true')
    p.add_argument('--imports-only', action='store_true')
    p.add_argument('--with-provenance', action='store_true', help='Add one source mapping per synthetic document before migration')
    p.add_argument('--workloads', nargs='+', choices=list(baseline_queries()), help='Only replace these query measurements in an existing report')
    p.add_argument('--matched-baseline', type=Path, help='Run baseline import immediately before unified import under identical current conditions')
    args = p.parse_args()
    report = {'phase': args.phase, 'source': str(args.source), 'fixture': 'synthetic-v1',
              'conditions': 'warm PostgreSQL, 21 repetitions, local Unix/TCP test connection, no HTTP/SMTP', 'sizes': {}}
    if (args.queries_only or args.imports_only) and args.output.exists():
        report = json.loads(args.output.read_text())
    queries = baseline_queries() if args.phase == 'baseline' else unified_queries()
    for size in args.sizes:
        item = report['sizes'].get(str(size), {})
        if not args.imports_only and args.matched_baseline:
            with temporary_database() as database:
                with CountedConnection.connect(database, cursor_factory=CountedCursor) as conn:
                    migrate(conn, args.matched_baseline)
                    seed(conn, size, args.with_provenance)
                    item.setdefault('matched_baseline_queries', {}).update({name: measure(conn, statements) for name, statements in baseline_queries().items() if not args.workloads or name in args.workloads})
        if not args.imports_only:
            with temporary_database() as database:
                with CountedConnection.connect(database, cursor_factory=CountedCursor) as conn:
                    migrate(conn, args.source, before=29 if args.phase == 'unified' else None)
                    seed(conn, size, args.with_provenance)
                    if args.phase == 'unified':
                        migrate(conn, args.source, after=29)
                        conn.execute('ANALYZE')
                        conn.commit()
                    item.setdefault('queries', {}).update({name: measure(conn, statements) for name, statements in queries.items() if not args.workloads or name in args.workloads})
                    item.setdefault('query_fixtures', {}).update({name: 'synthetic-v2-with-provenance' if args.with_provenance else 'synthetic-v1' for name in queries if not args.workloads or name in args.workloads})
        if not args.queries_only and args.matched_baseline:
            with temporary_database() as database:
                with CountedConnection.connect(database, cursor_factory=CountedCursor) as conn:
                    migrate(conn, args.matched_baseline)
                    item['matched_baseline_import'] = run_import(args.matched_baseline, conn, size // 2)
        if not args.queries_only:
            with temporary_database() as database:
                with CountedConnection.connect(database, cursor_factory=CountedCursor) as conn:
                    migrate(conn, args.source)
                    if args.phase == 'unified':
                        conn.execute("UPDATE document_import_sources SET mode='one_time',allow_publication=TRUE WHERE id='vismo:vysker.cz'")
                        conn.commit()
                    item['import'] = run_import(args.source, conn, size // 2)
        report['sizes'][str(size)] = item
        print(json.dumps({'size': size, 'queries': {k: {x: v for x,v in r.items() if x not in ('sql','plans')} for k,r in item.get('queries',{}).items()}, 'import': item.get('import'), 'matched_baseline_import': item.get('matched_baseline_import')}), flush=True)
    report['process_max_rss_mib'] = max(report.get('process_max_rss_mib', 0), round(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024, 1))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')


if __name__ == '__main__':
    main()
