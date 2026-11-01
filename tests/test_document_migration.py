"""Synthetic schema upgrade, byte preservation and immutable publication regressions."""
from datetime import date
import hashlib
import json
from pathlib import Path
import sys
import unittest
import tempfile
import psycopg

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from postgres import connect, temporary_database

PDF = b'%PDF-1.7\nMigration synthetic fixture\n%%EOF'
SHA = hashlib.sha256(PDF).hexdigest()


def baseline(conn):
    for path in sorted((ROOT / 'migrations').glob('*.sql')):
        if int(path.name.split('_', 1)[0]) < 29:
            conn.execute(path.read_text())
    conn.execute("INSERT INTO documents(id,title,description,status,created_at,published_at,source_published_on) VALUES (1,'Veřejný dokument','Původní popis','published','2020-01-01T10:00:00Z','2020-01-01T10:00:00Z','2020-01-01'),(2,'Skrytý dokument','Soukromý koncept','draft','2020-01-01T10:00:00Z',NULL,NULL)")
    conn.execute("INSERT INTO notices(id,title,description,category_id,published_on,published_at,withdraw_on,withdrawn_at,status,retain_attachments,review_json) VALUES (1,'Aktuální oznámení','Původní obsah',1,'2026-10-01','2026-10-01T10:00:00Z',NULL,NULL,'published',FALSE,'{}'),(2,'Historické oznámení','Historický obsah',1,'2020-01-01',NULL,'2020-01-15',NULL,'archived',TRUE,'{\"archive_basis\":\"Explicit synthetic test permission\",\"archive_until\":\"2099-12-31\"}'),(3,'Nejasná historie','Soukromá historie',NULL,NULL,NULL,NULL,NULL,'draft',FALSE,'{}')")
    for owner, parent, attachment in [('document', 1, 1), ('document', 2, 2), ('notice', 1, 3), ('notice', 2, 4), ('notice', 3, 5)]:
        conn.execute(f"INSERT INTO attachments(id,{owner}_id,name,content_type,size_bytes,data,sha256) VALUES (%s,%s,'test.pdf','application/pdf',%s,%s,%s)", (attachment, parent, len(PDF), PDF, SHA))
    for key, parent, attachment in [('doc:1', 1, 1), ('doc:2', 2, 2)]:
        conn.execute("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,attachment_id,destination) VALUES (%s,%s,%s,'2026-10-01T00:00:00Z','2026-10-01T00:00:00Z','{}',%s,%s,%s)", (key, 'https://source.test/'+key, SHA, parent, attachment, '/dokumenty/'+str(parent)))
    for key, parent, attachment in [('notice:2', 2, 4), ('notice:3', 3, 5)]:
        conn.execute("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,notice_id,attachment_id,destination) VALUES (%s,%s,%s,'2026-10-01T00:00:00Z','2026-10-01T00:00:00Z','{}',%s,%s,%s)", (key, 'https://source.test/'+key, SHA, parent, attachment, '/uredni-deska/'+str(parent)))
    conn.execute("INSERT INTO notice_events(notice_id,occurred_at,kind,payload,sha256) VALUES (1,'2026-10-01T10:00:00Z','published','{}',%s)", (SHA,))
    conn.execute("INSERT INTO consent_notices VALUES ('migration-fixture','v1','Synthetic consent','{}')")
    conn.execute("INSERT INTO subscribers(id,email,verified_at,retention_started_at) VALUES (1,'anonymous@example.test',1,1)")
    conn.execute("INSERT INTO subscription_consents(id,subscriber_id,notice_fingerprint,requested_at,confirmed_at) VALUES (1,1,'migration-fixture',1,1)")
    conn.execute("INSERT INTO publication_outbox(notice_id,subscriber_id,consent_id,created_at) VALUES (1,1,1,1)")
    conn.execute("INSERT INTO mail_queue(id,subscriber_id,purpose,subject,created_at,next_attempt_at,consent_id) VALUES (1,1,'document','Synthetic existing mail',1,1,1)")
    conn.execute("INSERT INTO mail_publication_items(mail_id,subscriber_id,notice_id) VALUES (1,1,2)")
    conn.commit()


class DocumentMigrationTests(unittest.TestCase):
    def test_collision_mapping_bytes_provenance_and_lock_guards(self):
        with temporary_database() as database, connect(database) as conn:
            baseline(conn)
            before = conn.execute("SELECT id,name,size_bytes,data,sha256 FROM attachments ORDER BY id").fetchall()
            for path in sorted((ROOT / 'migrations').glob('*.sql')):
                if int(path.name.split('_', 1)[0]) >= 29:
                    conn.execute(path.read_text())
            conn.commit()
            self.assertEqual(conn.execute("SELECT relkind FROM pg_class WHERE oid='documents'::regclass").fetchone()[0], 'r')
            self.assertEqual(conn.execute("SELECT relkind FROM pg_class WHERE oid='notices'::regclass").fetchone()[0], 'v')
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 5)
            mapping = {(model, old): new for model, old, new in conn.execute('SELECT old_model,old_id,document_id FROM document_legacy_ids')}
            self.assertEqual(mapping['document', 1], 1)
            self.assertEqual(mapping['document', 2], 2)
            self.assertEqual(len(set(mapping.values())), 5)
            active, historical, uncertain = (mapping['notice', i] for i in (1, 2, 3))
            self.assertEqual(conn.execute("SELECT id,name,size_bytes,data,sha256 FROM attachments ORDER BY id").fetchall(), before)
            self.assertEqual(conn.execute('SELECT document_id FROM attachments ORDER BY id').fetchall(), [(1,), (2,), (active,), (historical,), (uncertain,)])
            self.assertEqual(conn.execute('SELECT notice_id FROM notice_events').fetchone()[0], active)
            self.assertEqual(conn.execute('SELECT document_id,destination FROM legacy_sources WHERE source_key=%s', ('notice:2',)).fetchone(), (historical, f'/dokumenty/{historical}'))
            self.assertEqual(conn.execute('SELECT source_published_on FROM documents WHERE id=1').fetchone()[0], date(2020, 1, 1))
            self.assertEqual(conn.execute('SELECT lock_state,public_access FROM documents WHERE id=%s', (active,)).fetchone(), ('locked', True))
            self.assertEqual(conn.execute('SELECT lock_state,public_access FROM documents WHERE id=%s', (historical,)).fetchone(), ('locked', True))
            self.assertEqual(conn.execute('SELECT lock_state,public_access FROM documents WHERE id=%s', (uncertain,)).fetchone(), ('review', False))
            self.assertFalse(conn.execute('SELECT public_access FROM documents WHERE id=2').fetchone()[0])
            self.assertEqual(conn.execute('SELECT started_on,started_at,origin FROM document_postings WHERE document_id=%s', (historical,)).fetchone(), (date(2020, 1, 1), None, 'source'))
            self.assertEqual(conn.execute('SELECT notice_id,subscriber_id,consent_id FROM publication_outbox').fetchall(), [(active, 1, 1)])
            self.assertEqual(conn.execute('SELECT notice_id,mail_id FROM mail_publication_items').fetchall(), [(historical, 1)])
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 1)
            conn.commit()
            forbidden = [
                ("UPDATE documents SET title='Changed' WHERE id=%s", (active,)),
                ("UPDATE documents SET lock_state='draft' WHERE id=%s", (historical,)),
                ("UPDATE attachments SET data=%s WHERE id=3", (b'%PDF-1.7 different',)),
                ("UPDATE attachments SET sort_order=9 WHERE id=3", ()),
                ("DELETE FROM attachments WHERE id=3", ()),
                ("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES (%s,'new.pdf','application/pdf',%s,%s)", (active, len(PDF), PDF)),
                ("UPDATE documents SET title='Unverified edit' WHERE id=%s", (uncertain,)),
            ]
            for query, params in forbidden:
                with self.subTest(query=query):
                    with self.assertRaises(psycopg.Error) as error:
                        with conn.transaction():
                            conn.execute(query, params)
                    self.assertIn(error.exception.sqlstate, ('23514', 'P0001'))
            self.assertEqual(conn.execute('SELECT data,sha256 FROM attachments WHERE id=3').fetchone(), (PDF, SHA))
            # Visibility is independent and the lock persists when access ends.
            conn.execute("UPDATE documents SET public_access=FALSE,access_blocked=TRUE WHERE id=%s", (active,))
            self.assertEqual(conn.execute("SELECT document_is_public(d,DATE '2026-11-01'),lock_state FROM documents d WHERE id=%s", (active,)).fetchone(), (False, 'locked'))
            # New identifiers cannot collide with either legacy namespace.
            new_id = conn.execute("INSERT INTO documents(title,created_at) VALUES ('Nový koncept','2026-11-01T10:00:00Z') RETURNING id").fetchone()[0]
            self.assertGreater(new_id, max(mapping.values()))

    def test_operator_inventory_verifies_counts_checksums_links_and_no_new_mail(self):
        from document_migration_report import snapshot, compare
        import copy
        with temporary_database() as database, connect(database) as conn:
            baseline(conn)
            conn.execute("INSERT INTO documents(id,title,status,created_at,published_at) VALUES (3,'Noční zveřejnění','published','2020-03-04T23:30:00Z','2020-03-04T23:30:00Z')")
            conn.commit()
            before = snapshot(conn)
            for migration in sorted((ROOT / 'migrations').glob('*.sql')):
                if int(migration.name.split('_', 1)[0]) >= 29:
                    conn.execute(migration.read_text())
            conn.commit()
            after = snapshot(conn, before['as_of_date'])
            report = compare(before, after)
            self.assertTrue(report['valid'], report)
            self.assertEqual(report['counts']['after_canonical_documents'], 6)
            self.assertEqual(conn.execute('SELECT catalog_date FROM documents WHERE id=3').fetchone()[0], date(2020, 3, 5))
            encoded = json.dumps(before)
            self.assertNotIn('Veřejný dokument', encoded)
            self.assertNotIn('anonymous@example.test', encoded)
            self.assertNotIn('https://source.test', encoded)
            damaged = copy.deepcopy(after)
            damaged['attachments'][0]['bytes_sha256'] = 'different'
            self.assertIn('attachment_content_changed', compare(before, damaged)['discrepancy_counts'])
            damaged = copy.deepcopy(after)
            damaged['unchanged']['mail_queue'].append(dict(id=999,row_sha256='new'))
            self.assertIn('mail_or_subscriber_rows_changed', compare(before, damaged)['discrepancy_counts'])

    def test_dump_restore_rehearsal_preserves_original_schema_bytes_mail_and_consent(self):
        from document_migration_report import snapshot, compare
        from postgres import run_tool
        with temporary_database() as source, temporary_database() as restored, tempfile.TemporaryDirectory() as directory:
            with connect(source) as conn:
                baseline(conn)
                before = snapshot(conn)
            dump = Path(directory) / 'before-unified.dump'
            run_tool('pg_dump', source, '--format=custom', '--no-owner', '--no-privileges', '--file', str(dump))
            with connect(source) as conn:
                for migration in sorted((ROOT / 'migrations').glob('*.sql')):
                    if int(migration.name.split('_', 1)[0]) >= 29:
                        conn.execute(migration.read_text())
                conn.commit()
                self.assertTrue(compare(before,snapshot(conn,before['as_of_date']))['valid'])
            run_tool('pg_restore', restored, '--no-owner', '--no-privileges', '--exit-on-error', str(dump))
            with connect(restored) as conn:
                recovered = snapshot(conn,before['as_of_date'])
            self.assertEqual(recovered, before)
            self.assertEqual(len(recovered['unchanged']['subscribers']),1)
            self.assertEqual(len(recovered['unchanged']['subscription_consents']),1)
            self.assertEqual(len(recovered['unchanged']['mail_queue']),1)

    def test_future_source_dates_do_not_create_actual_historical_events(self):
        with temporary_database() as database, connect(database) as conn:
            baseline(conn)
            conn.execute("UPDATE notices SET withdraw_on='2030-01-15' WHERE id=2")
            conn.execute("UPDATE notices SET published_on='2030-01-01' WHERE id=3")
            conn.execute("""INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,
                imported_at,metadata,notice_id,destination) VALUES
                ('future-removal','https://source.test/future-removal',%s,'2026-10-01T00:00:00Z',
                 '2026-10-01T00:00:00Z',%s::jsonb,2,'/uredni-deska/2')""",
                (SHA,json.dumps(dict(dates=dict(published_on='2020-01-01',withdraw_on='2030-01-15',removed_on='2030-01-15')))))
            conn.commit()
            for migration in sorted((ROOT / 'migrations').glob('*.sql')):
                if int(migration.name.split('_', 1)[0]) >= 29:
                    conn.execute(migration.read_text())
            mapping = dict(conn.execute("SELECT old_id,document_id FROM document_legacy_ids WHERE old_model='notice'"))
            self.assertEqual(conn.execute('SELECT planned_removal_on,removed_on,removed_at FROM document_postings WHERE document_id=%s', (mapping[2],)).fetchone(), (date(2030,1,15),None,None))
            self.assertEqual(conn.execute('SELECT certainty,started_at FROM document_postings WHERE document_id=%s', (mapping[3],)).fetchone(), ('uncertain',None))
            self.assertEqual(conn.execute('SELECT lock_state FROM documents WHERE id=%s', (mapping[3],)).fetchone()[0], 'review')

    def test_existing_imports_cannot_be_announced_again_after_upgrade(self):
        from legacy import source_key
        from legacy_import import fingerprint, import_bundle
        from legacy_documents import configure_source
        with temporary_database() as database, connect(database) as conn, tempfile.TemporaryDirectory() as directory:
            baseline(conn)
            url = 'https://vysker.cz/assets/File.ashx?id_dokumenty=123&id_org=18774'
            key = source_key(url)
            item = dict(key=key, url=url, title='Veřejný dokument', name='test.pdf',
                        mime='application/pdf', parents=[], dates={'published_on': '2020-01-01'},
                        capture=dict(sha256=SHA, size=len(PDF), captured_at='2026-10-01T00:00:00Z'))
            conn.execute("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,attachment_id,destination) VALUES (%s,%s,%s,'2026-10-01T00:00:00Z','2026-10-01T00:00:00Z','{}',1,1,'/dokumenty/1')", (key, url, fingerprint(item)))
            conn.commit()
            for path in sorted((ROOT / 'migrations').glob('*.sql')):
                if int(path.name.split('_', 1)[0]) >= 29:
                    conn.execute(path.read_text())
            conn.commit()
            self.assertEqual(conn.execute('SELECT count(*) FROM document_announcements').fetchone()[0], 5)
            for (document_id,) in conn.execute('SELECT id FROM documents').fetchall():
                self.assertFalse(conn.execute('SELECT claim_document_announcement(%s,now())', (document_id,)).fetchone()[0])
            before = tuple(conn.execute('SELECT (SELECT count(*) FROM documents),(SELECT count(*) FROM attachments),(SELECT count(*) FROM document_postings),(SELECT count(*) FROM publication_outbox),(SELECT count(*) FROM mail_queue)').fetchone())
            self.assertEqual(before[-2:], (1, 1))
            root = Path(directory)
            (root / 'objects').mkdir()
            (root / 'objects' / SHA).write_bytes(PDF)
            (root / 'manifest.json').write_text(json.dumps(dict(version=1,origin='https://vysker.cz',pages=[],assets=[item],aliases={},errors=[],complete=True)))
            configure_source(conn, 'one_time', allow_publication=True, reason='Synthetic migration retry')
            for _ in range(2):
                result = import_bundle(conn, root, publish_content=True, sync=True)
                self.assertEqual(result['new'], [])
                self.assertEqual(result['unchanged'], [key])
                self.assertEqual(result['conflicts'], [])
                conn.commit()
            after = tuple(conn.execute('SELECT (SELECT count(*) FROM documents),(SELECT count(*) FROM attachments),(SELECT count(*) FROM document_postings),(SELECT count(*) FROM publication_outbox),(SELECT count(*) FROM mail_queue)').fetchone())
            self.assertEqual(after, before)
            self.assertEqual(conn.execute('SELECT count(*) FROM document_import_revisions').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT data,sha256 FROM attachments WHERE id=1').fetchone(), (PDF, SHA))

    def test_upgrade_preserves_effective_archive_privacy_and_source_date_precision(self):
        with temporary_database() as database, connect(database) as conn:
            baseline(conn)
            # Worker has not yet changed status, but the old public projection
            # already redacted an overdue native notice and denied its files.
            conn.execute("UPDATE notices SET withdraw_on=DATE '2020-01-15',published_on=DATE '2020-01-01',published_at='2020-01-01T10:00:00Z' WHERE id=1")
            conn.execute("UPDATE notices SET review_json=%s WHERE id=2", (json.dumps(dict(
                archive_title='Veřejný historický záznam', archive_basis='Synthetic explicit archive permission',
                archive_until='2099-12-31')),))
            # Provenance is protected once inserted. Add a second reliable source
            # record with explicit date-only actual removal for the archive.
            conn.execute("""INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,
                imported_at,metadata,notice_id,destination) VALUES
                ('archive-date-evidence','https://source.test/actual-dates',%s,
                 '2026-10-01T00:00:00Z','2026-10-01T00:00:00Z',%s::jsonb,2,'/uredni-deska/2')""",
                (SHA, json.dumps(dict(source_dates=dict(published_on='2020-01-01',withdraw_on='2020-01-15',removed_on='2020-01-16')))))
            conn.commit()
            for path in sorted((ROOT / 'migrations').glob('*.sql')):
                if int(path.name.split('_', 1)[0]) >= 29:
                    conn.execute(path.read_text())
            mapping = dict(conn.execute("SELECT old_id,document_id FROM document_legacy_ids WHERE old_model='notice'"))
            overdue, retained = mapping[1], mapping[2]
            self.assertEqual(conn.execute('SELECT metadata_restricted,attachments_public,public_access FROM documents WHERE id=%s',
                                          (overdue,)).fetchone(), (True, False, True))
            self.assertEqual(conn.execute('SELECT metadata_restricted,attachments_public,public_title FROM documents WHERE id=%s',
                                          (retained,)).fetchone(), (True, True, 'Veřejný historický záznam'))
            self.assertEqual(conn.execute('SELECT description FROM documents WHERE id=%s', (retained,)).fetchone()[0], 'Historický obsah')
            self.assertNotIn('historicky obsah', conn.execute('SELECT search_text FROM documents WHERE id=%s', (retained,)).fetchone()[0])
            self.assertEqual(conn.execute('SELECT removed_on,removed_at,planned_removal_on FROM document_postings WHERE document_id=%s',
                                          (retained,)).fetchone(), (date(2020, 1, 16), None, date(2020, 1, 15)))
            # The old overdue plan does not fabricate an actual removal event.
            self.assertEqual(conn.execute('SELECT removed_on,removed_at FROM document_postings WHERE document_id=%s',
                                          (overdue,)).fetchone(), (None, None))
            self.assertEqual(conn.execute('SELECT data,sha256 FROM attachments WHERE id=3').fetchone(), (PDF, SHA))

    def test_migrated_file_candidate_replaces_in_order_and_stays_accepted_on_retry(self):
        from legacy import digest, source_key
        from legacy_import import fingerprint, import_bundle
        from legacy_documents import decide_revision
        with temporary_database() as database, connect(database) as conn, tempfile.TemporaryDirectory() as directory:
            baseline(conn)
            url = 'https://vysker.cz/assets/File.ashx?id_dokumenty=777&id_org=18774'
            key = source_key(url)
            changed = b'%PDF-1.7\nChanged migration candidate\n%%EOF'
            changed_sha = digest(changed)
            proposal = dict(key=key,url=url,title='Changed source file',name='changed.pdf',
                mime='application/pdf',parents=[],dates={'published_on':'2020-01-01'},
                capture=dict(sha256=changed_sha,size=len(changed),captured_at='2026-10-01T00:00:00Z'))
            original_item = dict(proposal, capture=dict(proposal['capture'],sha256=SHA,size=len(PDF)))
            conn.execute("""INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,
                imported_at,metadata,notice_id,attachment_id,destination) VALUES
                (%s,%s,%s,'2026-10-01T00:00:00Z','2026-10-01T00:00:00Z','{}',2,4,'/api/v1/attachments/4')""",
                (key,url,fingerprint(original_item)))
            signature = digest(json.dumps([fingerprint(proposal),None,None]).encode())
            conn.execute("""INSERT INTO legacy_sync_reviews(source_key,fingerprint,proposal,source_data,first_seen_at,last_seen_at)
                VALUES (%s,%s,%s::jsonb,%s,'2026-10-01T00:00:00Z','2026-10-01T00:00:00Z')""",
                (key,signature,json.dumps(proposal),changed))
            conn.execute("""INSERT INTO attachments(id,notice_id,name,content_type,size_bytes,data,sha256,sort_order)
                VALUES (6,2,'other.pdf','application/pdf',%s,%s,%s,0)""", (len(PDF),PDF,SHA))
            # Explicit fixture IDs do not advance the old identity sequence.
            conn.execute("SELECT setval(pg_get_serial_sequence('attachments','id'),6,TRUE)")
            conn.commit()
            for path in sorted((ROOT / 'migrations').glob('*.sql')):
                if int(path.name.split('_',1)[0]) >= 29:
                    conn.execute(path.read_text())
            original_id = conn.execute("SELECT document_id FROM document_legacy_ids WHERE old_model='notice' AND old_id=2").fetchone()[0]
            original_files = conn.execute('SELECT id,name,data,sha256 FROM attachments WHERE document_id=%s ORDER BY sort_order,id', (original_id,)).fetchall()
            revision = conn.execute('SELECT id FROM document_import_revisions WHERE source_key=%s', (key,)).fetchone()[0]
            accepted = decide_revision(conn, revision, 'accept', 'Synthetic migrated revision review')['document_id']
            self.assertEqual(conn.execute('SELECT name FROM attachments WHERE document_id=%s ORDER BY sort_order,id', (accepted,)).fetchall(),
                             [('changed.pdf',),('other.pdf',)])
            self.assertEqual(conn.execute('SELECT id,name,data,sha256 FROM attachments WHERE document_id=%s ORDER BY sort_order,id', (original_id,)).fetchall(), original_files)
            self.assertEqual(conn.execute('SELECT public_access,source_published_on,board_status,description FROM documents WHERE id=%s', (accepted,)).fetchone(), (False,None,None,'Historický obsah'))
            root = Path(directory)
            (root / 'objects').mkdir()
            (root / 'objects' / changed_sha).write_bytes(changed)
            (root / 'manifest.json').write_text(json.dumps(dict(version=1,origin='https://vysker.cz',pages=[],assets=[proposal],aliases={},errors=[],complete=True)))
            for _ in range(2):
                report = import_bundle(conn, root, sync=True)
                self.assertEqual(report['conflicts'], [])
                self.assertEqual(report['unchanged'], [key])
            self.assertEqual(conn.execute('SELECT count(*) FROM document_import_revisions').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 6)


if __name__ == '__main__':
    unittest.main()
