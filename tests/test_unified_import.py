"""Anonymous PostgreSQL fixtures for immutable canonical source imports."""
import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from legacy import digest
from legacy_import import import_bundle, prepare
from legacy_documents import configure_source, decide_revision, refuse_destructive_reimport
from postgres import connect, temporary_database
from test_legacy import bundle, FILE, subscription_fixture
from legacy_notifications import queue_publication


@unittest.skipUnless(os.environ.get('TEST_DATABASE_URL'), 'Set TEST_DATABASE_URL or run scripts/test.sh')
class UnifiedImportTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.database = self.enterContext(temporary_database())
        with connect(self.database) as conn:
            for migration in sorted((Path(__file__).resolve().parents[1] / 'migrations').glob('*.sql')):
                conn.execute(migration.read_text())
        self.data = bundle(self.root, notice=True)

    def allow(self, conn):
        configure_source(conn, 'one_time', allow_publication=True, reason='Synthetic fixture archive decision')

    def save(self):
        (self.root / 'manifest.json').write_text(json.dumps(self.data))

    def replace_bytes(self, raw=b'%PDF-1.7\nchanged synthetic fixture\n%%EOF'):
        sha = digest(raw)
        (self.root / 'objects' / sha).write_bytes(raw)
        self.data['assets'][0]['capture'].update(sha256=sha, size=len(raw))
        self.save()

    def test_source_defaults_to_proposals_and_historical_import_never_announces(self):
        with connect(self.database) as conn:
            report = import_bundle(conn, self.root, publish_content=True, archive_notices=True)
            self.assertTrue(report['publication_blocked_by_source_policy'])
            self.assertEqual(conn.execute('SELECT public_access,lock_state FROM documents').fetchone(), (False, 'locked'))
            self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 0)
            posting = conn.execute('SELECT origin,certainty,started_on,started_at,removed_on,removed_at FROM document_postings').fetchone()
            self.assertEqual(posting[0:2], ('source', 'documented'))
            self.assertEqual(str(posting[2]), '2020-03-02')
            self.assertIsNone(posting[3])
            self.assertEqual(str(posting[4]), '2020-03-20')
            self.assertIsNone(posting[5])
            self.assertEqual(conn.execute('SELECT initial_state FROM document_postings').fetchone()[0], 'historical')
            self.assertEqual(conn.execute('SELECT has_current_posting,has_removed_posting FROM documents').fetchone(), (False, True))

    def test_board_and_catalog_have_one_content_row_and_repeat_is_unchanged(self):
        with connect(self.database) as conn:
            self.allow(conn)
            first = import_bundle(conn, self.root, archive_notices=True)
            original = conn.execute('SELECT id,title,content_sha256,public_access FROM documents').fetchone()
            self.assertTrue(original[3])
            self.assertEqual(conn.execute('SELECT id FROM notices').fetchone()[0], original[0])
            self.assertEqual(conn.execute('SELECT document_id,notice_id FROM attachments').fetchone(), (original[0], original[0]))
            self.assertEqual(conn.execute('SELECT document_fingerprint(id)=content_sha256 FROM documents').fetchone()[0], True)
            again = import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(again['new'], [])
            self.assertEqual(again['conflicts'], [])
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM document_postings').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 0)

    def test_changed_pdf_is_one_private_revision_and_rejection_stays_closed(self):
        with connect(self.database) as conn:
            self.allow(conn)
            import_bundle(conn, self.root, archive_notices=True)
            original = conn.execute('SELECT data,sha256 FROM attachments').fetchone()
            self.replace_bytes()
            first = import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(first['pending_reviews'], 1)
            revision = conn.execute('SELECT id FROM document_import_revisions').fetchone()[0]
            decide_revision(conn, revision, 'reject', 'Fixture rejection')
            second = import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(second['pending_reviews'], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM document_import_revisions').fetchone()[0], 1)
            self.assertEqual(conn.execute('SELECT data,sha256 FROM attachments').fetchone(), original)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 1)

    def test_acceptance_creates_private_linked_document_and_is_idempotent(self):
        with connect(self.database) as conn:
            self.allow(conn)
            import_bundle(conn, self.root, archive_notices=True)
            original = conn.execute('SELECT id,title,content_sha256 FROM documents').fetchone()
            original_bytes = conn.execute('SELECT data,sha256 FROM attachments').fetchone()
            self.replace_bytes()
            import_bundle(conn, self.root, archive_notices=True, sync=True)
            revision = conn.execute('SELECT id FROM document_import_revisions').fetchone()[0]
            accepted = decide_revision(conn, revision, 'accept', 'Fixture accepted as source observation')
            self.assertNotEqual(accepted['document_id'], original[0])
            self.assertEqual(conn.execute('SELECT lock_state,public_access,board_status FROM documents WHERE id=%s',
                                         (accepted['document_id'],)).fetchone(), ('draft', False, None))
            self.assertEqual(conn.execute('SELECT id,title,content_sha256 FROM documents WHERE id=%s', (original[0],)).fetchone(), original)
            self.assertEqual(conn.execute('SELECT data,sha256 FROM attachments WHERE document_id=%s', (original[0],)).fetchone(), original_bytes)
            self.assertEqual(conn.execute('SELECT original_id,related_id,kind FROM document_relations').fetchone(),
                             (original[0], accepted['document_id'], 'source_change'))
            repeated = import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(repeated['new'], [])
            self.assertEqual(repeated['conflicts'], [])
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 2)
            self.assertEqual(conn.execute('SELECT count(*) FROM document_postings').fetchone()[0], 1)

    def test_accepted_source_revision_keeps_friendly_url_changes_as_aliases(self):
        with connect(self.database) as conn:
            import_bundle(conn, self.root, archive_notices=True)
            self.data['pages'][0]['title'] = 'Změněný název zdrojové písemnosti'
            self.save()
            import_bundle(conn, self.root, archive_notices=True, sync=True)
            revision = conn.execute('SELECT id FROM document_import_revisions').fetchone()[0]
            decide_revision(conn, revision, 'accept', 'Synthetic accepted title change')
            self.data['pages'][0]['url'] = 'https://vysker.cz/jiny-nazev/ms-1053'
            self.save()
            repeated = import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(repeated['conflicts'], [])
            self.assertEqual(repeated['new'], [])
            self.assertEqual(conn.execute('SELECT count(*) FROM document_import_revisions').fetchone()[0], 1)
            self.assertTrue(conn.execute('SELECT EXISTS(SELECT 1 FROM document_source_aliases WHERE source_url=%s)',
                                        (self.data['pages'][0]['url'],)).fetchone()[0])

    def test_local_withdrawal_survives_sync_and_accepted_candidate(self):
        with connect(self.database) as conn:
            self.allow(conn)
            import_bundle(conn, self.root, archive_notices=True)
            doc = conn.execute('SELECT id FROM documents').fetchone()[0]
            conn.execute('UPDATE documents SET public_access=FALSE,access_blocked=TRUE WHERE id=%s', (doc,))
            self.replace_bytes()
            report = import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(len(report['blocked']), 2)
            revision = conn.execute('SELECT id FROM document_import_revisions').fetchone()[0]
            accepted = decide_revision(conn, revision, 'accept', 'Private fixture review')
            self.assertEqual(conn.execute('SELECT public_access,access_blocked FROM documents WHERE id=%s',
                                         (accepted['document_id'],)).fetchone(), (False, True))
            self.assertEqual(conn.execute('SELECT lock_state,public_access FROM documents WHERE id=%s', (doc,)).fetchone(), ('locked', False))

    def test_attachment_shared_by_distinct_notice_identities_cannot_publish_partial_records(self):
        duplicate = copy.deepcopy(self.data['pages'][0])
        duplicate.update(key='ms:987654',url='https://vysker.cz/druha-pisemnost/ms-987654',title='Druhá odlišná písemnost')
        self.data['pages'].append(duplicate)
        self.data['assets'][0]['parents'].append(duplicate['key'])
        self.save()
        with connect(self.database) as conn:
            self.allow(conn)
            report = import_bundle(conn,self.root,archive_notices=True)
            self.assertEqual(len(report['incomplete_documents']),2)
            self.assertTrue(all(item['ambiguous_attachments'] for item in report['incomplete_documents']))
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0],0)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0],0)

    def test_missing_notice_file_does_not_publish_partial_document(self):
        self.data['assets'][0]['error'] = 'HTTP 404'
        self.save()
        with connect(self.database) as conn:
            self.allow(conn)
            report = import_bundle(conn, self.root, archive_notices=True, allow_incomplete=True)
            self.assertEqual(len(report['incomplete_documents']), 1)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 0)

    def test_unknown_history_is_review_locked_without_invented_dates(self):
        self.data['pages'][0]['dates'] = {}
        self.data['assets'][0].update(dates={}, evidence='')
        self.save()
        with connect(self.database) as conn:
            self.allow(conn)
            import_bundle(conn, self.root, archive_notices=True)
            self.assertEqual(conn.execute('SELECT lock_state,public_access,source_published_on FROM documents').fetchone(), ('review', False, None))
            self.assertEqual(conn.execute('SELECT certainty,started_on,started_at FROM document_postings').fetchone(), ('uncertain', None, None))

    def test_extra_attachment_is_proposed_without_modifying_locked_list(self):
        with connect(self.database) as conn:
            self.allow(conn)
            import_bundle(conn, self.root, archive_notices=True)
            extra = copy.deepcopy(self.data['assets'][0])
            extra.update(url=FILE.replace('123', '999'), name='extra.pdf', title='New attachment')
            self.data['assets'].append(extra)
            self.data['pages'][0]['assets'].append(dict(url=extra['url'], title=extra['title']))
            self.save()
            report = import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(conn.execute('SELECT count(*) FROM attachments').fetchone()[0], 1)
            self.assertGreaterEqual(report['pending_reviews'], 1)
            count = conn.execute('SELECT count(*) FROM document_import_revisions').fetchone()[0]
            import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(conn.execute('SELECT count(*) FROM document_import_revisions').fetchone()[0], count)

    def test_later_subscribers_and_direct_queue_cannot_reannounce_imported_history(self):
        with connect(self.database) as conn:
            self.allow(conn)
            import_bundle(conn, self.root, archive_notices=True)
            document = conn.execute('SELECT id FROM documents').fetchone()[0]
            subscription_fixture(conn)
            # Every enqueue path checks the permanent canonical announcement ledger.
            queue_publication(conn, '2026-11-01T12:00:00Z', notice_id=document)
            queue_publication(conn, '2026-11-01T12:00:00Z', document_id=document)
            import_bundle(conn, self.root, archive_notices=True, sync=True)
            self.assertEqual(conn.execute('SELECT count(*) FROM publication_outbox').fetchone()[0], 0)
            self.assertEqual(conn.execute('SELECT count(*) FROM mail_queue').fetchone()[0], 0)

    def test_planned_or_future_source_removal_never_becomes_an_actual_event(self):
        for removal, archive in [(None, True), ('2030-03-20', True), ('2030-03-20', False)]:
            with self.subTest(removal=removal, archive=archive), temporary_database() as database, connect(database) as conn, tempfile.TemporaryDirectory() as directory:
                for migration in sorted((Path(__file__).resolve().parents[1] / 'migrations').glob('*.sql')):
                    conn.execute(migration.read_text())
                data = bundle(Path(directory), notice=True)
                data['pages'][0]['dates'] = dict(published_on='2020-03-02', withdraw_on='2030-03-20')
                if removal:
                    data['pages'][0]['dates']['removed_on'] = removal
                if not archive:
                    data['pages'][0]['kind'] = 'page'
                    data['assets'][0]['dates'] = dict(data['pages'][0]['dates'])
                    data['assets'][0]['evidence'] = 'Vyvěšeno: 2. 3. 2020. Sejmuto: 20. 3. 2030.'
                    original = Path(directory) / 'objects' / data['pages'][0]['capture']['sha256']
                    content = '<p class="cesta"><a>Úřední deska</a></p>'.encode() + original.read_bytes()
                    sha = digest(content)
                    (Path(directory) / 'objects' / sha).write_bytes(content)
                    data['pages'][0]['capture'].update(sha256=sha,size=len(content))
                (Path(directory) / 'manifest.json').write_text(json.dumps(data))
                configure_source(conn, 'one_time', allow_publication=True, reason='Synthetic source plan review')
                with patch.dict(os.environ, {'OBEC_VEREJNA_URL': 'http://127.0.0.1:3000', 'OBEC_DATABAZE': database, 'OBEC_PRODUCTION': 'false'}):
                    import_bundle(conn, directory, archive_notices=archive, preview_notices=not archive, classify_navigation=not archive)
                actual = conn.execute('SELECT planned_removal_on,removed_on,removed_at FROM document_postings').fetchone()
                self.assertEqual(tuple(str(v) if v else None for v in actual), ('2030-03-20',None,None))
                self.assertEqual(conn.execute('SELECT public_access FROM documents').fetchone()[0], not archive)
                self.assertEqual(conn.execute('SELECT initial_state FROM document_postings').fetchone()[0], 'historical' if archive else 'current')
                self.assertEqual(conn.execute('SELECT has_current_posting,has_removed_posting FROM documents').fetchone(), (not archive, archive))

    def test_destructive_reimport_has_no_unlock_escape_hatch(self):
        with connect(self.database) as conn:
            import_bundle(conn, self.root, archive_notices=True)
            with self.assertRaisesRegex(ValueError, 'Destructive reimport is disabled'):
                refuse_destructive_reimport(conn)
            self.assertEqual(conn.execute('SELECT count(*) FROM documents').fetchone()[0], 1)


if __name__ == '__main__':
    unittest.main()
