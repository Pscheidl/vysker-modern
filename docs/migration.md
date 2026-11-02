# Importing the original Vyskeř website

The existing Python capture and PostgreSQL importer remain the migration tools.
They use Beautiful Soup and psycopg from the project Nix environment. All document
content now lives in `documents`, including records displayed on the notice board.
`notices` is a compatibility view over that same table. A source capture is not a
local posting and does not prove the historical bytes of a physical notice board.

## Source authority and handover

Migration `0029_unified_documents.sql` creates the explicit source
`vismo:vysker.cz`, initially in `proposals` mode with publication disabled. Existing
public records keep their migrated access settings. New records stay private
until an operator records the intended source policy. A configured source never
has authority to reverse local withdrawal, edit locked content or delete data.

Configure the source in the target database using `OBEC_DATABAZE` or
`OBEC_DATABAZE_FILE`. Credentials do not belong in command arguments or Git.

```sh
python3 scripts/legacy_sync.py configure --mode one_time \
  --allow-publication --reason 'Reviewed one-time historical migration'
```

Supported modes are `one_time`, `mirror`, `proposals` and `disabled`.
`--allow-publication` allows publication requested by the import flags. It is not
an instruction to republish existing records. Periodic `run` requires `mirror`
or `proposals`. `one_time` remains safely repeatable through the offline importer.
The mode and authority changes are audited. Operational authority is not supported
by this additive importer, so remote dates never overwrite local posting or
withdrawal decisions.

Before the old domain starts serving the new application, disable the source:

```sh
python3 scripts/legacy_sync.py configure --mode disabled \
  --reason 'Local administration has taken over the official website'
```

Stop the old synchronization timer as part of handover. The runner also refuses
to crawl a source whose hostname is the configured local public hostname. Domain
names alone do not grant source authority or select the handover mode.

## Two-phase capture and validation

```sh
python3 scripts/legacy.py capture --bundle data/migration/vysker-snapshot
```

The crawler downloads public Vismo pages and files only from `vysker.cz` and
`www.vysker.cz`. It does not submit forms, send email or follow administration
links. Requests have bounded sizes, delays, timeouts and retry limits. Default
limits are 1,500 pages and 5,000 files. Offline imports additionally reject manifests
over 64 MiB or 20,000 identities before loading objects. Split larger captures
into bundles containing complete documents and all their required attachments.
These bounds and one-file-at-a-time byte reads cap memory independently of archive
size. Objects are addressed by SHA-256 in a
private offline bundle. The manifest holds identities, source URLs, original
dates, capture times, extraction warnings and failures. Reusing a bundle resumes
that capture. A new directory starts a fresh observation.

Every fresh periodic crawl revalidates existing file URLs. HTTP validators may
return 304, in which case the verified cached bytes are reused. Without a valid
validator, the file is downloaded and hashed again. `--refresh-assets` forces an
unconditional download. A stable URL never means immutable bytes. HTML and file
network errors remain visible and never silently substitute stale content.

HTTP 404 resources remain in the report and are retried on fresh captures. Missing
items, an incomplete list, failed requests or disappearance from navigation do not
delete or withdraw a local document. `--allow-incomplete` imports only the usable
subset after operator review. A notice with a missing required attachment is held
for review as a whole, so successful siblings cannot create a partial public
notice. Existing complete content survives failures of later observations.

Temporary run bundles and superseded unreferenced cache objects are pruned. The
application does not require storing all original HTML indefinitely. Keep reviewed
migration evidence under the municipality's separate retention policy. Private
state under ignored `data/` must not be committed or published as CI artifacts.

## Plan, dry-run and import

Apply application migrations on a disposable database first. Validate all source
objects before applying a reviewed migration to any operational database.

```sh
python3 scripts/legacy_import.py dry-run \
  --bundle data/migration/vysker-snapshot --classify-navigation \
  --archive-notices --report data/migration/plan.json

python3 scripts/legacy_import.py import \
  --bundle data/migration/vysker-snapshot --classify-navigation \
  --notice-map config/legacy-vysker-notices.json \
  --page-map config/legacy-vysker-pages.json \
  --publish-content --archive-notices --report data/migration/import.json
```

`plan` is an alias for the read-only dry-run. Reports distinguish new, unchanged,
conflicting, locally blocked, incomplete and failed records. Import validates the
bundle before a local evidence transaction. The shared application writer lock,
source uniqueness and revision uniqueness serialize concurrent imports. Failure
rolls back the whole evidence transaction. Retry cannot duplicate a committed
record. Successful runs have database run records, counts and timestamps. A file
claimed by multiple distinct structured notice identities holds those new records
for ownership review, so none can publish with a partial attachment list. Multiple
ordinary source listings of the same stable item still map to one record.

Source mappings are keyed by the explicit source plus the Vismo source identity.
Where Vismo has no entity ID, the canonical source URL is the fallback identity.
File query parameters identifying a Vismo document remain significant. Names,
list positions and byte hashes are not identities. URL aliases are retained in
`document_source_aliases`. Identical PDFs from different source identities remain
different documents. Existing attachment IDs and their authenticated delivery
routes stay stable.

`--publish-content` requests ordinary document/page/calendar publication within
the configured source authority. `--archive-notices` requests continued historical
availability, without creating a local posting. Automatic archive availability
requires known source posting and actual removal dates. Missing dates stay null
and the content stays private for review. A source deadline is not removal.
New captures retain explicit `Sejmuto` label evidence as `metadata.dates.removed_on`
without changing the existing semantic fingerprint. Actual removal must be no
later than the source capture date. An unlabeled legacy `withdraw_on`, a future
`removed_on`, or a date merely passing never proves actual removal.
Historical documents and notices do not queue subscriber announcements, including
when newly imported or explicitly moved into the historical archive. The permanent
announcement ledger also prevents already imported records from being announced
again after migration, withdrawal or later source observations. Existing calendar
notification behavior is separate and unchanged.

`--classify-navigation` uses the source breadcrumb to identify board history. It
does not make a category into a posting event. Board source records are finalized
only after their attachments have been validated and inserted. Documented history
locks content permanently. Uncertain history gets a conservative `review` lock,
without asserting a documented posting. Both preserve capture time and source
dates separately from any local event. A captured historical PDF is described as
a copy captured during import, not a verified historical original.
The posting's immutable `initial_state` records whether it was taken as current,
historical or uncertain. Historical source records remain archival when their
actual removal date is unknown. A later local posting and its removal leave that
source history unchanged. Source dates are never completed from the local worker's
clock or merely from an expired planned deadline.

`--skip-pages` excludes content pages, galleries and unrelated images while
retaining documents, board records and calendar entries. Container pages linking
to documents are omitted by the existing classification. The optional page map
retains reviewed contact and mandatory-information slugs. Gallery aliases still
resolve to their owning article, and verified page images use their existing
controlled media endpoint. Calendar dates retain their known/unknown precision.
`--preview-notices` remains restricted to isolated local previews and cannot
establish local publication evidence.

## Periodic synchronization and immutable revisions

```sh
python3 scripts/legacy_sync.py configure --mode mirror \
  --allow-publication --reason 'Approved temporary parallel operation'
python3 scripts/legacy_sync.py run --state data/migration/sync \
  --publish-content --archive-notices --skip-pages \
  --notice-map config/legacy-vysker-notices.json
```

The runner holds a separate nonblocking synchronization lock while acquiring a
fresh capture. A concurrent runner exits with `status: skipped`. The crawl never
holds a database transaction open. Its `last-run.json`, `last-success.json`,
`capture-errors.json` and `capture-unavailable.json` distinguish failed capture,
committed import and failed report export. A retry after report failure is safe.

With an explicit source policy allowing publication, `--archive-notices` makes
verified imported archive copies available in Documents even when the source
does not state posting or removal dates. Unknown dates and uncertain source
history remain unknown. Public availability of the captured document is separate
from evidence of a historical posting or removal. The captured content remains
immutable and historical imports do not enqueue mail.

To correct an existing source archive copy withheld only by the former date
requirement, use the targeted operation after checking the source document:

```sh
python3 scripts/legacy_sync.py publish-archive --document-id DOCUMENT_ID \
  --reason 'Verified public source copy withheld by the previous import date requirement'
```

This verifies provenance, immutable content and attachment integrity, respects
the source publication policy and local access decisions, and records the
availability correction. It does not change posting dates, unlock content,
create another document or send a publication notice. It is safe to repeat.

Unchanged content does not change public records, duplicate attachments, create
postings or queue mail. Observation metadata is updated in bounded batches of
500 rows. Mapping lookups select the current bundle's keys. Lists and reports do
not load attachment bytes for all records into memory.

Changed title, text, evidence or file bytes create an immutable private candidate
in `document_import_revisions`. Candidate files are stored separately with hashes,
order and visibility flags. They have no public URL. Their fingerprint deduplicates
repeated observations, including previously rejected revisions. The old
`legacy_sync_reviews` table is only a compatibility projection for operator exports.
The revision ledger retains every observed candidate and final decision.

```sh
python3 scripts/legacy_sync.py reviews --output data/migration/review
python3 scripts/legacy_sync.py decide --revision-id 42 --decision reject \
  --reason 'Internal review explanation'
python3 scripts/legacy_sync.py decide --revision-id 43 --decision accept \
  --reason 'Reviewed as a new source observation'
```

The same decision function is used by administration and the command line.
Acceptance creates a new private concept with a new ID and a `source_change`
relationship. It does not edit the original, claim an issuer correction, create
a posting or start a legal period. Candidate snapshots include the complete
observed attachment list, so a later accepted change cannot bring back an earlier
sibling PDF. Local access blocks and private file flags are preserved. Incomplete
candidates cannot be accepted. A later unchanged import recognizes the accepted
fingerprint and does not recreate or revert that concept.

`acknowledge --source-key ... --fingerprint ...` remains a compatibility command
that rejects the exact pending observation without accepting content. A stale
identifier is refused. Internal reasons and candidate bytes are not public
history.

## Historical links and verification

```sh
python3 scripts/legacy_verify.py --base-url http://127.0.0.1:3012 \
  --report data/migration/verification.json
```

The verifier checks current canonical visibility, direct legacy redirects and
hashes of public attachment downloads. Original Vismo paths and source aliases
may redirect only while their target is public. Attachment routes independently
check access on every request, including old URLs. Files restricted after removal
remain available to authorized administrators. A public index or cached link
never grants access to hidden bytes.

The old `legacy_reconcile.py`, `legacy_reimport.py` and `legacy_replace.py`
destructive paths refuse the canonical schema before disabling preservation
triggers or removing content. They cannot be used as an unlock operation. Use
additive import and reviewed new documents. A rollback is a controlled restore of
a complete verified backup with all writers stopped, not deletion of the current
source mapping followed by reimport.

For the canonical schema migration, counts, ID collision mapping, indexes,
performance measurements and backup/return instructions, see the unified document
[verification report](unified-documents-verification.md). No production deployment is performed by these tests.
Website checks do not verify lawful publication on the physical notice board.

## Host transfer

For a planned host transfer stop every writer, mail worker and importer before
`pg_dump`, restore the complete snapshot to the new host, verify counts, hashes,
private downloads and outgoing queue, and start only the new instance. Preserve
subscriber consent and the announcement ledger. Do not use a restore mode that
discards subscriptions for a planned transfer. Files are stored in PostgreSQL,
so a complete database backup includes document and private candidate bytes.
Keep the old instance stopped to avoid independent publication and delivery.
