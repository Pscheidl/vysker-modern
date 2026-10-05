"""Historical archive conversion on disposable PostgreSQL databases."""
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import digest
from legacy_archive import archive_imported_notices, plan_archive
from legacy_scope import archive_review
from postgres import connect, temporary_database
from test_legacy import subscription_fixture


STAMP = '2026-10-01T12:00:00+00:00'
RAW = b'%PDF-1.7\nfixture\n%%EOF'
WRITE_LOCK = "hashtextextended(current_schema() || ':vysker-write', 0)"


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class ArchiveTests(unittest.TestCase):
    def setUp(self):
        self.database = temporary_database()
        self.url = self.database.__enter__()
        with connect(self.url) as conn:
            for migration in sorted((Path(__file__).resolve().parents[1] / 'migrations').glob('*.sql')):
                conn.execute(migration.read_text())

    def tearDown(self):
        self.database.__exit__(None, None, None)

    def imported(self, conn, status='draft', native=False, attachment=True, posted=None, end=None):
        notice_id = conn.execute('''INSERT INTO notices(title,description,status,published_on,
            withdraw_on,created_at,updated_at,review_json) VALUES (%s,%s,%s,%s,%s,%s,%s,%s)
            RETURNING id''', ('Původní zápis', 'Původní popis', status, posted, end, STAMP, STAMP,
                              json.dumps({'original_reference': 'https://vysker.cz/puvodni'}))).fetchone()[0]
        attachment_id = None
        if attachment:
            attachment_id = conn.execute('''INSERT INTO attachments(notice_id,name,content_type,size_bytes,data,sha256)
                VALUES (%s,'zapis.pdf','application/pdf',%s,%s,%s) RETURNING id''',
                (notice_id, len(RAW), RAW, digest(RAW))).fetchone()[0]
        if native:
            return notice_id, attachment_id
        key = f'/assets/File.ashx?id_dokumenty={notice_id}&id_org=18774'
        metadata = json.dumps({'capture': {'sha256': digest(RAW)}, 'original_title': 'Původní zápis'})
        conn.execute('''INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,
            imported_at,metadata,notice_id,attachment_id,destination) VALUES (%s,%s,%s,%s,%s,%s::jsonb,%s,%s,%s)''',
            (key, 'https://vysker.cz' + key, digest(RAW), STAMP, STAMP, metadata,
             notice_id, attachment_id, f'/uredni-deska/{notice_id}'))
        conn.execute('''INSERT INTO legacy_notice_imports(source_key,notice_id,imported_at,metadata)
            VALUES (%s,%s,%s,%s::jsonb)''', (key, notice_id, STAMP, metadata))
        for kind, entity_id in [('notice', notice_id), ('attachment', attachment_id)]:
            if entity_id:
                conn.execute('''INSERT INTO audit_log(occurred_at,operation,entity_type,entity_id)
                    VALUES (%s,'legacy_imported',%s,%s)''', (STAMP, kind, entity_id))
        return notice_id, attachment_id

    def snapshot(self, conn, tables):
        return {table: conn.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall() for table in tables}

    def test_read_only_plan_and_archive_preserve_sources_dates_files_and_other_content(self):
        unchanged = ('legacy_sources', 'legacy_notice_imports', 'attachments',
                     'pages', 'documents', 'events', 'notice_events', 'mail_queue')
        with connect(self.url) as conn:
            subscribers = subscription_fixture(conn)
            unknown, _ = self.imported(conn)
            dated, _ = self.imported(conn, posted='2020-03-02', end='2020-03-20')
            text_only, _ = self.imported(conn, attachment=False)
            conn.execute("INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('kontakt','Kontakt','Text',TRUE,%s)", (STAMP,))
            conn.execute("INSERT INTO documents(title,status,created_at) VALUES ('Dokument','published',%s)", (STAMP,))
            conn.execute("INSERT INTO events(title,starts_at,ends_at,published,updated_at) VALUES ('Akce',%s,%s,TRUE,%s)", (STAMP, STAMP, STAMP))
            before = self.snapshot(conn, unchanged)
            dates = conn.execute('SELECT id,published_on,withdraw_on,published_at,withdrawn_at FROM notices ORDER BY id').fetchall()
        with connect(self.url) as conn:
            conn.execute('SET TRANSACTION READ ONLY')
            plan = plan_archive(conn)
            self.assertEqual(plan['candidate_ids'], [unknown, dated, text_only])
            self.assertEqual(plan['archived_count'], 0)
            self.assertEqual(self.snapshot(conn, unchanged), before)
            self.assertEqual(conn.execute("SELECT count(*) FROM notices WHERE status='draft'").fetchone()[0], 3)
        with connect(self.url) as conn:
            report = archive_imported_notices(conn, 3)
            self.assertEqual(report['archived_ids'], [unknown, dated, text_only])
            self.assertEqual(self.snapshot(conn, unchanged), before)
            self.assertEqual(conn.execute('SELECT id,published_on,withdraw_on,published_at,withdrawn_at FROM notices ORDER BY id').fetchall(), dates)
            for status, retain, review in conn.execute('SELECT status,retain_attachments,review_json FROM notices'):
                self.assertEqual(status, 'archived')
                self.assertTrue(retain)
                self.assertEqual(json.loads(review), dict(original_reference='https://vysker.cz/puvodni', **archive_review('Původní zápis')))
            operations = conn.execute("SELECT operation,count(*) FROM audit_log WHERE entity_type='notice' GROUP BY operation ORDER BY operation").fetchall()
            self.assertEqual(operations, [('legacy_archived', 3), ('legacy_imported', 3)])
            self.assertEqual(conn.execute('''SELECT notice_id,document_id,subscriber_id,consent_id
                FROM publication_outbox ORDER BY id''').fetchall(),
                [(notice_id, None, *subscribers[name]) for notice_id in (unknown, dated, text_only)
                 for name in ('active', 'active_other')])

    def test_native_scheduled_edited_and_previously_published_records_are_excluded(self):
        with connect(self.url) as conn:
            eligible, _ = self.imported(conn)
            native, _ = self.imported(conn, native=True)
            scheduled, _ = self.imported(conn, status='scheduled', posted='2030-01-01')
            edited, _ = self.imported(conn)
            conn.execute("INSERT INTO audit_log(operation,entity_type,entity_id) VALUES ('updated','notice',%s)", (edited,))
            changed, _ = self.imported(conn)
            conn.execute("UPDATE notices SET updated_at='2026-10-02T12:00:00+00:00' WHERE id=%s", (changed,))
            published, _ = self.imported(conn, posted='2020-01-01')
            conn.execute('UPDATE notices SET published_at=%s WHERE id=%s', (STAMP, published))
            withdrawn, _ = self.imported(conn)
            conn.execute('UPDATE notices SET withdrawn_at=%s WHERE id=%s', (STAMP, withdrawn))
            evidence, _ = self.imported(conn)
            conn.execute("INSERT INTO notice_events(notice_id,occurred_at,kind,payload,sha256) VALUES (%s,%s,'published','{}',%s)", (evidence, STAMP, digest(b'{}')))
            added_file, attachment_id = self.imported(conn)
            conn.execute("INSERT INTO audit_log(operation,entity_type,entity_id) VALUES ('uploaded','attachment',%s)", (attachment_id,))
            before = conn.execute('SELECT * FROM notices WHERE id<>%s ORDER BY id', (eligible,)).fetchall()
            report = archive_imported_notices(conn, 1)
            self.assertEqual(report['candidate_ids'], [eligible])
            excluded = {row['id']: row['reasons'] for row in report['excluded']}
            for notice_id, reason in [(scheduled, 'status_scheduled'), (edited, 'notice_edits'),
                                     (changed, 'changed_timestamp'), (published, 'publication_history'),
                                     (withdrawn, 'publication_history'), (evidence, 'has_events'),
                                     (added_file, 'attachment_edits')]:
                self.assertIn(reason, excluded[notice_id])
            self.assertNotIn(native, excluded)
            self.assertEqual(conn.execute('SELECT * FROM notices WHERE id<>%s ORDER BY id', (eligible,)).fetchall(), before)

    def test_missing_removed_and_corrupt_attachments_are_excluded(self):
        with connect(self.url) as conn:
            eligible, _ = self.imported(conn)
            missing, attachment_id = self.imported(conn)
            conn.execute('UPDATE attachments SET data=NULL WHERE id=%s', (attachment_id,))
            removed, attachment_id = self.imported(conn)
            conn.execute('UPDATE attachments SET data=NULL,removed_at=%s WHERE id=%s', (STAMP, attachment_id))
            corrupt, attachment_id = self.imported(conn)
            conn.execute('UPDATE attachments SET data=%s WHERE id=%s', (b'x' * len(RAW), attachment_id))
            wrong_size, attachment_id = self.imported(conn)
            conn.execute('UPDATE attachments SET size_bytes=size_bytes+1 WHERE id=%s', (attachment_id,))
            report = plan_archive(conn)
            self.assertEqual(report['candidate_ids'], [eligible])
            excluded = {row['id']: row['reasons'] for row in report['excluded']}
            for notice_id in (missing, removed, corrupt, wrong_size):
                self.assertIn('unavailable_attachment', excluded[notice_id])

    def test_expected_count_guard_and_failure_roll_back_the_whole_conversion(self):
        with connect(self.url) as conn:
            subscription_fixture(conn)
            self.imported(conn)
            self.imported(conn)
            before = self.snapshot(conn, ('notices', 'audit_log', 'publication_outbox'))
        with self.assertRaisesRegex(ValueError, 'Expected 1.*found 2'), connect(self.url) as conn:
            archive_imported_notices(conn, 1)
        with self.assertRaisesRegex(RuntimeError, 'forced failure'), connect(self.url) as conn:
            with patch('legacy_archive.archive_review', side_effect=[archive_review('Původní zápis'), RuntimeError('forced failure')]):
                archive_imported_notices(conn, 2)
        with connect(self.url) as conn:
            self.assertEqual(self.snapshot(conn, tuple(before)), before)

    def test_idempotence_and_standard_write_lock(self):
        with connect(self.url) as conn:
            subscribers = subscription_fixture(conn)
            self.imported(conn)
        with connect(self.url) as conn, connect(self.url, autocommit=True) as concurrent:
            archive_imported_notices(conn, 1)
            self.assertFalse(concurrent.execute(f'SELECT pg_try_advisory_lock({WRITE_LOCK})').fetchone()[0])
        with connect(self.url) as conn:
            conn.execute('UPDATE subscribers SET verified_at=4 WHERE id=%s', (subscribers['pending'][0],))
            conn.execute('UPDATE subscription_consents SET confirmed_at=4 WHERE id=%s', (subscribers['pending'][1],))
            self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 2)
            before = self.snapshot(conn, ('notices', 'audit_log', 'legacy_sources', 'attachments', 'publication_outbox'))
            report = archive_imported_notices(conn, 0)
            self.assertEqual(report['archived_count'], 0)
            self.assertEqual(report['archived_ids'], [])
            self.assertEqual(self.snapshot(conn, tuple(before)), before)


if __name__ == '__main__':
    unittest.main()
