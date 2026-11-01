# Unified document verification

This report concerns local implementation and synthetic test data only. No live
records, subscriber data, production deployment or physical notice board were
accessed or changed. Test success does not establish legal publication.

## Baseline and repeatable workload

The baseline is Git commit `7c9211a` from `origin/main`. Before implementation,
that source was exported to `/tmp/vysker-documents-baseline`. The machine's
installed PostgreSQL 18.6 and Python 3.14.7/psycopg 3.3.4 were used. The temporary
worktree's development PostgreSQL server hosts disposable, randomly named test
databases. The harness never opens `OBEC_DATABAZE` or an application's database.

```sh
TEST_DATABASE_URL=postgresql://vysker:vysker@127.0.0.1:5432/postgres \
python3 scripts/document_benchmark.py --phase baseline \
  --source /tmp/vysker-documents-baseline \
  --output docs/benchmarks/documents-before.json
```

The 2,000 and 20,000 document fixtures contain equal numbers of ordinary documents
and old notice records, colliding IDs between those tables, two tiny synthetic
attachments per document, public/private and current/archive states, known/unknown
dates, and 20/200 public pages. The larger fixture is ten times the smaller one.
It does not claim to reproduce the unknown current production distribution.

Each read workload executes the application's count and first-page SQL with a
20-row page, or the document-detail queries, after a warm-up. The initial baseline
detail measured its three core queries. The final matched detail measures the
complete SSR path, including the new posting, correction and relation queries,
and adds one provenance mapping per document on both versions. The report
stores median and p95 of 21 calls, statement count, Python allocation peak and
`EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` for every query. PostgreSQL plans include
actual row counts, buffer accesses, scan and sort strategies. Timings include
Python/driver overhead, with allocation tracking active, and exclude HTTP,
rendering, SMTP and network downloads. They are local comparisons, not production
latency promises. Read-query memory is bounded by the selected page.

The import workload calls the real version's `import_bundle` over a validated
synthetic offline bundle of 1,000/10,000 stable source identities. It measures
initial import, identical repeat, a PDF replacement under the same source URL,
and an identical repeat of that replacement. It records all client SQL executions
through `Cursor.execute` and peak traced Python allocations. Internal SQL inside
PostgreSQL functions/triggers is reflected in timings, but not counted as extra
client statements. No subscribers are created and no mail
can be sent. The payloads are deliberately small so memory reflects metadata
and importer behavior rather than a 500 MB real library.

## Before the change

| Workload | 2,000 documents median | 20,000 documents median | SQL statements |
| --- | ---: | ---: | ---: |
| Catalogue | 1.419 ms | 9.586 ms | 2 |
| Current board | 0.592 ms | 2.672 ms | 2 |
| Archive | 2.463 ms | 19.659 ms | 2 |
| Global `rozpocet` search | 18.008 ms | 124.901 ms | 2 |
| Document detail | 0.421 ms | 0.436 ms | 3 |

| Import | 1,000 items | SQL | 10,000 items | SQL |
| --- | ---: | ---: | ---: | ---: |
| Initial | 3.965 s | 7,005 | 39.879 s | 70,005 |
| Identical repeat | 0.923 s | 5 | 9.915 s | 5 |
| One changed PDF | 0.866 s | 6 | 10.538 s | 6 |
| Same changed PDF again | 0.892 s | 6 | 9.573 s | 6 |

Import peak tracked Python allocation was 31.83 MiB and 45.75 MiB respectively.
Process peak RSS across the complete baseline was 280 MiB, including driver,
Python, module imports, metadata and allocator retention. SQL read allocation
peaks ranged from 4.4 to 19.6 KiB.

The old current-board plan uses `notices_by_status`. Catalogue and archive plans
scan their eligible rows and use top-N sorting. Detail uses primary-key and
attachment-owner indexes. Search scans eligible records, repeatedly normalizes
Czech characters and combines the two content tables plus pages. It is the
largest measured read cost.

## Original unified-model measurements

The final comparison runs exported baseline and current implementations under
the same conditions. Builds and other test jobs were paused for the list and
import timings. Full raw statements, timings, memory peaks and plans are in
`benchmarks/documents-after.json`, with `matched_baseline_queries` and
`matched_baseline_import` preserving the accompanying old-version measurements.
The original pre-implementation report remains in `documents-before.json`.

| List, two SQL statements | 2,000 before | 2,000 after | 20,000 before | 20,000 after |
| --- | ---: | ---: | ---: | ---: |
| Catalogue | 1.677 ms | 1.195 ms | 9.712 ms | 6.504 ms |
| Current board | 0.759 ms | 0.589 ms | 2.863 ms | 0.944 ms |
| Public archive | 2.799 ms | 0.775 ms | 17.854 ms | 4.163 ms |
| Global `rozpocet` search | 21.432 ms | 4.130 ms | 136.130 ms | 29.252 ms |
| Selective `anonymni 9999` search | 23.931 ms | 2.354 ms | 161.969 ms | 4.506 ms |

At 20,000 documents the corresponding new p95 times are 7.405, 2.286, 4.540,
36.896 and 5.764 ms. The new catalogue covers both old content tables and remains
faster. There is no material list regression. Read allocation peaks are between
8.1 and 16.2 KiB for those first-page results. All counts and pagination execute
in PostgreSQL, and only the selected summaries cross into application memory.

| Complete SSR detail with provenance | Before, 3 SQL | After, 6 SQL | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: |
| 2,000 documents/source mappings | 0.579 ms | 0.863 ms | 1.009 ms | 2.947 ms |
| 20,000 documents/source mappings | 1.980 ms | 0.979 ms | 2.607 ms | 3.396 ms |

The complete new detail additionally loads posting history, evidence corrections
and document relations. That adds three fixed queries and 0.284 ms at the small
scale. It does not add per-attachment or per-posting queries. At the larger scale,
the new detail is faster because provenance resolves indexed source keys rather
than scanning all mappings. Its final plan uses `legacy_sources_document` and
`legacy_sources_pkey`, with the empty revision branch correctly choosing a tiny
sequential scan. An accepted revision lookup has its own partial document-ID
index. New detail allocation peaks at 7.1 KiB. The p95 increase reflects additional
round trips, while total median time remains about one millisecond at both scales.
This small fixed cost is the explicit tradeoff for the newly displayed evidence.

```sh
TEST_DATABASE_URL=postgresql://vysker:vysker@127.0.0.1:5432/postgres \
python3 scripts/document_benchmark.py --phase unified --queries-only \
  --workloads document_detail --with-provenance \
  --matched-baseline /tmp/vysker-documents-baseline \
  --output docs/benchmarks/documents-after.json
```

| Import | 1,000 before | 1,000 after | SQL before / after | 10,000 before | 10,000 after | SQL before / after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Initial | 3.398 s | 3.534 s | 7,005 / 5,016 | 39.081 s | 34.969 s | 70,005 / 50,052 |
| Identical repeat | 0.866 s | 0.922 s | 5 / 16 | 10.204 s | 9.941 s | 5 / 52 |
| One changed PDF | 0.890 s | 0.969 s | 6 / 24 | 10.389 s | 9.364 s | 6 / 60 |
| Same changed PDF again | 0.940 s | 0.971 s | 6 / 21 | 10.328 s | 10.530 s | 6 / 57 |

The 1,000-item runs add 56 to 136 ms for validated source policy, local blocks,
permanent announcement checks, observation history and audit. At 10,000 items,
initial import improves by 10.5 percent, unchanged import by 2.6 percent and one
changed file by 9.9 percent. Repeating the same conflict adds 203 ms, or 2 percent,
without creating a second revision. No public content is rewritten on an
identical repeat. The extra repeat statements are bounded metadata batches of
500 items, not per-document queries. Initial import uses fewer client statements.

Peak traced allocation for unchanged runs remains 31.71 MiB and 45.73 MiB. The
new initial 10,000-item run peaks at 53.79 MiB and the one-change run at 59.46 MiB,
including the captured proposal, source inventory and fingerprints. Files are
processed one at a time, and inventory size remains governed by the capture
limits. The complete measured process peaked at 279.4 MiB RSS. This measurement
does not include PostgreSQL shared buffers or make a claim about large PDF memory.

The existing importer keeps one complete bundle atomic under the existing
schema-wide writer advisory lock. Batches of 500 apply to metadata statements,
not separate commits. The new importer validates captured bytes before acquiring
that lock, while the baseline validated under it. Both versions can make an
editorial write exceed its existing ten-second lock timeout during a large first
import. MVCC public reads continue. Use smaller complete-document bundles and a
maintenance window for initial historical imports. The explicit limits are
20,000 identities and a 64 MiB manifest per bundle. Changing commit boundaries
requires preserving owner/attachment completeness and is not implied by the
metadata batching in this change.

```sh
TEST_DATABASE_URL=postgresql://vysker:vysker@127.0.0.1:5432/postgres \
python3 scripts/document_benchmark.py --phase unified \
  --matched-baseline /tmp/vysker-documents-baseline \
  --output docs/benchmarks/documents-after.json
```

## Migration rehearsal

```sh
TEST_DATABASE_URL=postgresql://vysker:vysker@127.0.0.1:5432/postgres \
python3 tests/test_document_migration.py
```

The fixture applies original migrations through 0028, inserts two ordinary
documents and three notice records with ID collisions, then applies the single migration 0029. It
asserts that all five content records occupy `documents`, while `notices` is a
compatibility view of those same rows. Original ordinary IDs remain unchanged,
old notice IDs map to distinct canonical IDs, and the next generated ID cannot
collide with either namespace.

The five attachments retain their IDs, names, bytes, lengths and SHA-256 values.
All source mappings, attachment ownership, notice evidence and existing mail
references are checked. Historical source dates remain date-only, with no invented
publication timestamp. A known source posting becomes locked, an uncertain
import becomes conservatively protected for review, and previously private
records remain private. Direct SQL content edits, unlocking, attachment uploads,
replacement, reordering and deletion fail. Ending public access leaves the lock
intact. Existing notification work is preserved without inserting new messages.

A second fixture has a confirmed synthetic subscriber and existing imported
content before upgrade. All five old documents gain permanent announcement
ledger entries. Claiming another announcement returns false for each one.
Reimporting the same source identity twice creates no new content, attachments,
posting, import revision, outbox request or mail. Existing outbox/mail counts
remain exactly one each.

Additional fixtures preserve legacy overdue-board privacy before the scheduled
worker runs, retain the source's date-only precision, and replace a migrated
file candidate in its original attachment order. Accepting that candidate
creates a new related document, and the next import recognizes the accepted
state without duplicating or reverting it. Future source dates remain planned
dates, not actual historical events. A separate fixture exercises the operator
inventory report, and a real `pg_dump`/`pg_restore` rehearsal verifies the original
schema, bytes, pending mail and subscriber consent after rollback. All seven
migration/restore tests passed in the combined Python run of 122 tests, which
completed in 42.785 seconds.

Production migration requires a separate reviewed maintenance window and verified
backup, with every application, importer and mail worker stopped. The test uses
anonymous fixtures and does not resolve ambiguous real-world source records.

## Query changes and indexes

The public catalogue now queries the canonical `documents` table. Notice-board
membership uses `has_current_posting` and `has_removed_posting` as transactional
projections of actual documented entries in `document_postings`. PostgreSQL
triggers maintain those fields in the same transaction as posting changes and
recompute them if a writer attempts to set them directly. Plans and dates alone
cannot create membership. The history stays authoritative.

A first measured implementation checked history with semi-joins for every count
and page. At 20,000 records that took 6.275 ms for the current board and 34.368 ms
for the archive, versus 2.672 ms and 19.659 ms before the change. That regression
was addressed before delivery with the transactional projections and ordered
partial indexes. No materialized view, external search service or access cache
was introduced.

| Index | Purpose and observed plan |
| --- | --- |
| `documents_catalog` | Public, unblocked `catalog_date DESC NULLS LAST,id DESC`, ordered index scan for catalogue page |
| `documents_current_board` | Public, unblocked actual current postings, ordered index scans for count and page |
| `documents_public_archive` | Public, unblocked ordinary archive or actual removed postings, no current/scheduled posting, ordered `catalog_date,id` page scan |
| `documents_topic` | Category and original date for thematic filtering |
| `documents_board`, `documents_due` | Board status and scheduled maintenance projections |
| `documents_search_public` | Partial GIN using trusted PostgreSQL `pg_trgm`, bitmap index scan for selective substring searches |
| `document_postings_document`, `document_postings_current`, `document_postings_removed` | Authoritative history lookup and transactional projection refresh |
| `legacy_sources_document`, `document_legacy_ids_document` | Detail provenance and legacy identity resolution |
| `document_events_document`, `document_relations_related` | Bounded history and incoming document relations |
| `legacy_sources_identity` | Unique source plus stable source identity |
| `document_import_revisions_pending`, `document_import_revisions_document`, `document_import_revisions_accepted` | Review queue, document provenance and accepted-source state |
| `document_revisions_accepted_document` | Resolve an accepted revision's provenance by its new document ID |

`search_text` is derived only from public-content fields and the current topic.
It is refreshed when those values change. A topic rename refreshes affected
rows without changing the locked source content or snapshot. Normalization uses
explicit Czech character mapping, and bound LIKE parameters escape `%`, `_` and
backslash to preserve literal search semantics. A common word matching most
rows correctly uses a sequential scan, while a selective query uses the GIN
index. Every result still requires current document visibility in the same SQL
statement, so stale search text cannot expose a withdrawn document.

Migrated restricted archive metadata contributes only its approved public title
and topic to search. Original title, body, reference number and issuer stay
private. File release separately evaluates `document_files_public` on every
request, including an expired archive deadline before a maintenance worker runs.
The compact board projection uses that same rule. There is no public content
cache whose invalidation is required for these controls to take effect.

The complete catalogue intentionally has more eligible rows than the old
ordinary-document-only catalogue. The new archive intentionally excludes items
without documented removal or current permission for public access. The fixtures
are identical, but these corrected result sets are part of the requested change.
Count and result page use one repeatable-read snapshot in the Rust application.
Lists fetch at most 20 summaries and never attachment bytes or complete history.
Homepage categories are joined in one query and its compact rows omit attachment
lookups. The legacy notice-list helper now fetches page IDs in bulk.

The test suite also covers literal Czech searches, stable unknown-date ordering,
pagination through 1,025 records, one result for a shared catalogue/board document,
separate corrective documents, immediate access revocation despite unchanged
search text, topic rename refresh and public-page inclusion.

## Additional security regression coverage

`tests/document_security.rs` exercises the actual routers and shared SSR loader
with synthetic PDF/image bytes and source URL mappings. It covers GET and HEAD
on direct files, old source redirects, the old board URL, public media, public
detail/list APIs and all search scopes after full withdrawal. The stored search
text deliberately remains unchanged to prove access does not depend on reindex.
Authenticated cookies do not make a public file URL bypass its public policy.

File-only restriction keeps permitted metadata and admin access, while hiding
all file links and the original source-file URL from SSR. An individually private
attachment also cannot disclose its external source URL through provenance.
Provenance still records that the document was imported. Private revision files,
previews, history, downloads and all added registry mutations require the
existing authenticated administration. An uncertain historical removal does not
appear as a documented entry through the older public board API. Deleted draft
attachments keep their internal metadata but never expose their original
filenames through the later public detail.

Latest verification covers 289 passing Rust tests and seven tests intentionally
ignored by the suite: five require Mailpit, one contacts the live ČEZ API, and one
is an opt-in digest load test. The full run passed 288 tests and found one obsolete
privacy fixture that treated archival as hiding. After changing that fixture to
the explicit visibility operation, the focused registry and privacy suites passed
12 and 11 tests. All eight search tests then passed again, including Prague-date
conversion after the final migration edit.

The combined Python suite passed 122 tests in 34.971 seconds. After the final
Prague-date change, all seven migration/backup-restore tests passed again in
3.873 seconds. All 12 affected browser scenarios passed in 1.5 minutes, including
the simplified editor, shared archive, ID collision redirect, file privacy and
keyboard-operated mobile help. The final SQL and import measurements are recorded
in the follow-up section below.

## Controlled upgrade and rollback

This is a prepared procedure, not authorization or a record of a production
upgrade. First rehearse on a protected copy of the deployment's database and
review its actual volume and `document_migration_issues`. The synthetic rehearsal
cannot classify uncertain municipal records or prove their historical bytes.

1. Record the running release identifier and database connection configuration.
   Stop the web/API application, scheduled import services and timers, mail
   workers and other writers. Keep the maintenance page active for the entire
   validation window. Old and new application versions must not write concurrently.
2. Set `OBEC_DATABAZE_FILE` to the protected connection file for that database.
   Record the before-inventory with `scripts/document_migration_report.py`,
   then create a complete custom-format backup in a dedicated directory:

   ```sh
   python3 scripts/document_migration_report.py snapshot --output data/migration/before.json
   python3 scripts/database.py backup --directory data/backups/unified-documents --keep-days 30
   ```

   Keep the exact printed backup path, release image/binary, protected configuration
   and encryption key. Document bytes and private candidate files are in PostgreSQL,
   so this complete backup includes them. Keep offline source bundles separately
   under their existing access and retention policy.
3. Restore this backup into a **new** rehearsal database and verify it before the
   operational upgrade. `postgres.create_database` refuses an existing database.
   The following preservation restore is for this stopped-writer upgrade window,
   not recovery of an old snapshot after subsequent subscriber withdrawals:

   ```sh
   MIGRATION_BACKUP_PATH=data/backups/unified-documents/EXACT-SNAPSHOT.dump \
   ROLLBACK_DATABASE_FILE=/protected/path/new-database-connection \
   PYTHONPATH=scripts python3 - <<'PY'
   import os
   from pathlib import Path
   from postgres import create_database, run_tool
   target = Path(os.environ['ROLLBACK_DATABASE_FILE']).read_text().strip()
   create_database(target)
   run_tool('pg_restore', target, '--single-transaction', '--exit-on-error',
            '--no-owner', '--no-acl', os.environ['MIGRATION_BACKUP_PATH'])
   PY
   ```

   Use the restored database's connection file to run the before-inventory again
   and compare it with the saved inventory. Never use `--discard-subscriptions`
   for this controlled upgrade or host transfer. Its separate disaster-recovery
   behavior intentionally removes subscribers and is unsuitable here.
4. With the new release and the intended connection file, apply schema changes
   without starting workers:

   ```sh
   ./obec-admin migrate
   python3 scripts/document_migration_report.py verify \
     --before data/migration/before.json --output data/migration/after.json
   ```

   The single migration 0029 runs through the existing SQLx migration ledger and
   contains the canonical model, import revisions, retention and announcement
   protections in the same controlled schema transition.
   A failed migration transaction rolls back, and completed checksummed versions
   are not applied twice. PostgreSQL needs the trusted `pg_trgm` extension.
   The merge and byte hashing take a bounded maintenance window, rather than
   attempting unsafe concurrent old/new writes. Rehearsal on real-sized private
   data determines that window. No per-file network traffic is required.
5. Verify all inventory differences, old aliases, current/public archive membership,
   private direct file rejection, authenticated downloads and the announcement
   ledger. Pending mail and subscriber consents must match the before-inventory.
   Uncertain histories and possible duplicate identities are reported for review,
   never automatically merged or republished. Do not resume a source whose
   policy has not been explicitly reviewed for the handover.
6. Start only the new release after validation. Observe readiness and the existing
   maintenance/mail monitoring. Historical documents remain suppressed by
   `document_announcements` even if subscribers joined later. Existing pending
   mail retains its identity and is rechecked against current access before send.

To roll back before reopening writes, leave all workers stopped, restore the
verified pre-upgrade snapshot into another new database using step 3, point the
old release at it and repeat the before-inventory checks. Keep the failed upgraded
copy for diagnosis. Do not run an inverse merge, reset IDs, drop source mappings,
remove announcement markers or delete SQLx migration records. Once writes have
resumed, the old backup is no longer a lossless rollback. Stop and reconcile the
new records and consent changes before any recovery decision.

## Self-review and additional security review

The implementation received a separate security review after the functional
self-review. Findings were fixed before delivery and covered by regression tests:

- Public document details no longer expose a source-file link when the document's
  files or that individual attachment are private, unavailable or removed.
- An anonymized rendition needs its own current preview token and complete files.
  Its exact bytes become immutable before the original's removal can release it.
  This creates no claim that the rendition was previously posted. A later local
  access block still prevents release.
- A generic source deadline is not an actual removal event. Future claimed dates
  remain uncertain, and incomplete archived evidence is reported for review.
- The compatibility board API uses the same documented current/archive projection
  as the catalogue and search.
- Old individual notification messages also recheck current document access.
  Digest regeneration uses the current permitted public title.
- Database conflict handling is scoped to the document lifecycle guards, preserving
  existing error and rollback behavior elsewhere in the application.

`tests/document_security.rs` checks anonymous GET and HEAD requests against direct
attachment URLs, old Vismo file aliases, the image endpoint, old board routes,
public API, SSR and search with unchanged index text. A private file returns no
bytes through those routes. Merely sending an administrator cookie to a public
file URL does not bypass the rule. The separate administrator endpoint requires
an active authenticated account and continues to return the retained original.
Both public and administrator file responses use `no-store`.

`tests/document_registry.rs` additionally checks mandatory no-default access
selection, stale previews, concurrent edit versus posting, immutable metadata
and files, repeated posting, corrections, incomplete attachments, incident
removal and locally blocked schedules. Python tests cover same-URL byte changes,
private candidate revisions, acceptance into a new record, rejected revision
idempotence and existing subscriber announcement suppression.

Large initial imports retain the existing whole-bundle transaction and shared
writer lock. The 500-item operations are SQL metadata batches, not separate
commits. Readers retain MVCC access, but an administrative write can reach the
existing 10-second lock timeout during a large import. Split large initial loads
into smaller complete-document bundles and use the reviewed maintenance window.
This contention behavior existed before the change, and the matched initial
import time improved. The validated manifest and identity limits bound each run.

The existing operational checks also passed: two backup/restore tests, ten
monitoring tests and five release-packaging tests. Packaging includes the new
canonical import module and migration-report command in both supported release
formats. The local preview was then started on a separate database with synthetic
records and SMTP capture mode. Its homepage returned 200, a direct public request
for the private archived attachment returned 404, and the authenticated
administrator download returned the original bytes.

## One editor and one archive follow-up

The revised administration has one **Dokumenty** section. An optional posting
checkbox reveals the posting date, blank required number of days and blank
required attachment policy. Reference number and creation date are optional.
The saved settings are reviewed once, with no parallel notice editor or
mandatory issuer/legal-profile fields. Existing import provenance and internal
evidence remain stored. The public detail distinguishes the actual document
creation date from original source publication and local import time. A local
publication timestamp used for catalogue ordering is converted to its Prague
calendar day, including a UTC timestamp near midnight. Explicit creation or
source dates retain precedence.

The catalogue's `/dokumenty?state=archive` is the only public archive. Its database
predicate is an ordinary archived record without board state, or a documented
removed posting, with no active posting and no scheduled reposting. A wholly
hidden record is still excluded by the shared access function. Incomplete source
history does not become an archive entry merely because its old status says
archived. Ordinary archival preserves public metadata and permitted files and
does not create a posting/removal event. The current board and homepage link to
this shared archive, and old board archive query URLs lead there with the query
preserved. Global search still returns each document once.

`tests/search.rs` adds shared-archive membership and separate creation/source-date
regressions. Browser scenarios check the single editor/navigation, blank mandatory
posting settings, ordinary archive file retention, worker-recorded removal with
private retained bytes, both former board archive query variants, migrated administrator ID collisions,
and keyboard-operated help in a narrow viewport.

### Final measurements after the follow-up

The final SQL comparison reran the exported original main branch and the current
implementation on the same anonymous 2,000/20,000-document fixtures. Our builds,
browser runs and other test jobs were paused. All six paths use current access
guards and date projections. Full SQL, paired timings, plans and allocation peaks
are in [documents-shared-archive.json](benchmarks/documents-shared-archive.json).
Detail fixtures additionally contain one source mapping per document. The original
comparison above remains a historical checkpoint, not the final query timings.

| Path | 2,000 before | 2,000 final | 20,000 before | 20,000 final | Final 20,000 p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Catalogue | 1.251 ms | 0.774 ms | 8.659 ms | 4.919 ms | 6.726 ms |
| Current board | 0.593 ms | 0.557 ms | 2.097 ms | 0.763 ms | 1.458 ms |
| Shared public archive | 2.096 ms | 0.825 ms | 15.771 ms | 5.691 ms | 6.586 ms |
| Global `rozpocet` search | 17.072 ms | 3.019 ms | 125.663 ms | 28.627 ms | 30.529 ms |
| Selective `anonymni 9999` search | 21.321 ms | 1.455 ms | 159.616 ms | 3.008 ms | 4.312 ms |
| Complete detail with provenance | 0.519 ms | 0.855 ms | 1.779 ms | 0.996 ms | 2.995 ms |

All five list/search paths still execute two data queries and return at most 20
summaries. Transaction control and outer page rendering are outside this SQL
harness, as in the original comparison. The complete detail executes six fixed
queries instead of the original three, for the additional posting history,
evidence corrections and relations. Its 2,000-record fixed cost increases by
0.336 ms. At 20,000 source mappings it remains faster than the original lookup.
Query-client allocation peaks are 7.1 to 17.4 KiB. These are traced Python client
allocations, not an estimate of the full Rust server process or PostgreSQL cache.

The revised `documents_public_archive` partial index uses the shared archive
predicate and `catalog_date DESC NULLS LAST,id DESC`. The 20,000-record page plan
now reads 21 qualifying rows using 37 shared buffer hits. Before the index change,
the same new query used the catalogue index, filtered out 837 rows and used 2,229
buffer hits. Count still chooses a sequential scan with 8,000 of 20,000 documents
qualifying. This explains the remaining archive cost without extra application
queries or loading records into memory. Adding `INCLUDE(public_until)` did not
change that plan or improve the diagnostic run, so it was not retained. The final
archive remains substantially faster than the original two-model implementation.

These unchanged legacy fixtures represent the migration comparison. Membership of
newly archived ordinary documents, scheduled reposting exclusion and uncertain
source history are independently covered by the shared-archive regression tests.

The final 10,000-item import was also rerun on the final migration, including its
revised archive index and Prague-date trigger. This was a fresh current-only run.
Its comparison uses the already recorded original baseline from
`documents-after.json`, rather than claiming another paired baseline run. Raw
results and that reference are in
[documents-final-import.json](benchmarks/documents-final-import.json).

| 10,000 source items | Recorded original baseline | Fresh final import | SQL before/final |
| --- | ---: | ---: | ---: |
| Initial import | 39.081 s | 31.793 s | 70,005 / 50,052 |
| Identical repeat | 10.204 s | 8.744 s | 5 / 52 |
| One changed PDF | 10.389 s | 8.923 s | 6 / 60 |
| Same changed PDF again | 10.328 s | 8.957 s | 6 / 57 |

The identical repeat reports zero new documents and zero conflicts. Changing one
PDF reports one pending review, and observing the same changed bytes again still
reports one pending review. The increased repeat query count consists of bounded
source/observation batches and explicit policy, local-block and revision checks.
It does not introduce per-document detail reads or a full public reindex.

Traced import allocation peaks at 64.14 MiB, compared with 45.75 MiB in the recorded
original baseline. The unified importer keeps additional validated source,
revision and identity metadata. This is a bounded working-memory cost, not a
claim of memory improvement. The fresh current process peaks at 155.3 MiB RSS.
The earlier combined-process RSS figure is not a directly paired comparison.
Each bundle is still limited to 20,000 identities and a 64 MiB manifest. File bytes
are processed one at a time, while the complete bundle commits atomically as
described above. None of the final import phases is slower than its recorded original baseline.

### Final local preview verification

The final preview uses a new isolated database containing seven synthetic
documents. Its SQLx ledger ends at migration 29, with no later migration and a
checksum matching the current 0029 file. `documents` is a physical table and
`notices` is only a compatibility view. The selected private attachment returns
404 through its public URL and 200 through authenticated administration. The
catalogue, shared archive and canonical detail return `no-store` and `noindex`.
Both `mail_queue` and `publication_outbox` remain empty. This is local preview
verification only. No production records or deployment were changed, and login
credentials are intentionally excluded from this repository.

The final SSR/WASM and administrator builds passed. The separate demo WASM
configuration also passed `cargo check --no-default-features --features demo
--lib --target wasm32-unknown-unknown`. Formatting and whitespace checks passed.
