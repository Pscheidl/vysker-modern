# Importing the original Vyskeř website

The migration tools capture the public Vismo website into an offline bundle, then
import it into PostgreSQL. The capture follows the menu and `#stred` content
discovery approach of Pavel's existing `/home/pavel/dev/scrape` project and adds
XML sitemap discovery, content extraction, file downloads and checksums.
It does not run the old scraper or its email notification worker.

The Rust website still uses Leptos, Axum and SQLx. These one-off operator tools
use Python, Beautiful Soup and psycopg, already included in the Nix environment.

The first local rehearsal is documented in [migration-report.md](migration-report.md).

## Capture

```sh
nix develop
python3 scripts/legacy.py capture --bundle data/migration/vysker-snapshot
```

The crawler only downloads public content from `vysker.cz` and `www.vysker.cz`.
Redirects outside those hosts are rejected. It never submits forms, votes, sends
messages or follows administration links. Requests have a delay, timeouts,
bounded response sizes and limited retries. Page and file limits are explicit.

The bundle contains:

- `manifest.json`: normalized identities, source URLs, text, dates, asset links,
  source metadata, warnings, errors and capture timestamps
- `objects/`: original HTML and file bytes addressed by SHA-256
- `responses/`: response metadata used to resume interrupted downloads
- `summary.json`: coverage and byte counts

Reusing the same directory resumes the same snapshot, including cached responses.
Use a **new directory** for a fresh capture before the final switch. Download
times and sitemap modification dates are never treated as publication dates.
The bundle contains municipal documents and belongs in controlled storage. The
repository ignores `data/`, so it is not included in commits or demo builds.

HTTP 404 responses for content pages, attachments and the sitemap are skipped
without blocking the rest of the import. Their source/final URLs, resource kind, status and check
time remain in `manifest.json` and import reports under `unavailable_resources`.
The failed item retains its metadata and error, but has no imported content or
attachment bytes. The surrounding captured page and its original link are
preserved. Every fresh run requests these resources again, so restored resources
are captured normally. Other HTTP statuses, transport failures, size limits and
invalid file signatures still make capture fail. If the sitemap returns HTTP 404,
discovery continues from the homepage, notice-board seeds and captured page links.

## Plan and import

Create a separate PostgreSQL database and start the current application once
against it to apply SQLx migrations. Use local mode and keep that preparation
instance bound to localhost. Do not point the migration at an existing production
database without a reviewed backup and reconciliation plan.

Provide the target connection through `OBEC_DATABAZE` or `OBEC_DATABAZE_FILE`.
Keep credentials out of command arguments and Git.

```sh
python3 scripts/legacy_import.py plan \
  --bundle data/migration/vysker-snapshot \
  --classify-navigation \
  --report data/migration/plan.json

python3 scripts/legacy_import.py import \
  --bundle data/migration/vysker-snapshot \
  --page-map config/legacy-vysker-pages.json \
  --classify-navigation \
  --notice-map config/legacy-vysker-notices.json \
  --report data/migration/import.json
```

Planning runs in a read-only transaction. Import validates all referenced objects
and writes one database transaction under the application's write lock. It creates
drafts by default. A repeated import skips unchanged identities. Changed source
records cause a conflict before any writes, including when an editor has already
changed the imported record. The importer never overwrites local edits.

Content pages linking to successfully captured documents are omitted. Only their
files are imported, with the usual document or notice classification. This applies
even when the page also contains text, and to import, sync and local library
reimport alike. Verified image attachments alone do not exclude a page. Structured
calendar entries and notices keep their existing import behavior.

The report lists these pages in `skipped_document_pages`, with their source URLs,
document identities and `already_imported` flag. Omitted pages do not get new page
records, revisions or redirects. Links to omitted pages remain on the original
website and appear in `unresolved_internal_links` for editorial review. File URLs
keep their normal attachment redirects. Repeated imports still deduplicate files.
Previously imported pages and local edits are retained, with the usual conflict
checks. Removing those existing pages requires a separate reviewed cleanup.

`--skip-pages` limits new imports to documents, notices and calendar entries.
Content pages, gallery pages and their aliases are omitted, including their
source-change proposals. The crawler still reads HTML to find files and preserve
source dates, titles and notice-board ownership. Gallery and illustration images
are omitted unless they are document downloads or belong to an imported notice
or calendar entry. This option is supported by both `legacy_import.py` and
`legacy_sync.py run`. It does not delete content imported by earlier runs.

`--publish-content` publishes ordinary pages, documents and calendar entries for
a reviewed test environment. Notices remain drafts unless `--archive-notices`
is also selected for a public historical archive. This option does not send
subscription notifications. It also does not publish previously imported drafts
on a repeated run. Use the administration for subsequent editorial decisions.

Incomplete captures are refused by default. After reading the capture errors,
`--allow-incomplete` allows the successfully captured subset to be imported.
Failures remain in the import report. This flag is **not** a claim of complete
coverage, and missing documents must be reconciled before launch.

## Periodic synchronization while both websites run

`legacy_sync.py run` is the cron entry point. The original website remains the
source for new items during parallel operation. Every run builds a fresh
inventory, rechecks HTML pages and reuses previously verified attachment bytes.
It imports new identities and records
changed identities for human review. It never updates or deletes existing
content, including when an item disappears from the original site. Local edits,
publication decisions and attachment URLs remain intact.

Known attachments are reused by canonical source URL, including document ID query
parameters, without an HTTP request or download delay. The private persistent
`capture-cache` directory stores response metadata and content-addressed objects.
On a cache miss, existing imported attachments can seed it from PostgreSQL, one
file at a time. Original response metadata is preserved and bytes are checked
against their size, SHA-256 and file signature. Missing or corrupt cache entries
are repaired from the database or downloaded again. Failed downloads and HTTP 404
are not cached as successful files, so restored resources are checked next time.

HTML pages and the sitemap are always requested again, using `ETag` or
`Last-Modified` validators when the source provides them. HTTP 304 reuses verified
HTML bytes. This discovers new attachments added to old pages as well as new
pages. Transport and server errors never silently substitute old HTML. Unreferenced
old object versions are pruned after a complete crawl, while cache entries for
previously seen attachment URLs remain available even if their links disappear.

The default intentionally does not detect replacement bytes at an unchanged
attachment URL. Add `--refresh-assets` to the same `legacy_sync.py run` command
when a full attachment refresh is needed. It bypasses both file cache and database
reuse. Changed bytes are proposed for review, without replacing published originals.

The database primary key on `legacy_sources.source_key` and the application's
transactional write lock prevent duplicate inserts, including concurrent imports
and retries after interruption. A separate nonblocking database lock covers the
whole scheduled crawl. If another sync is already running, the job exits
successfully with `status: skipped` without downloading anything.

Changed source content, file bytes detected during a refresh, attachment descriptions and notice ownership
are kept in `legacy_sync_reviews`, with at most one current proposal per original
identity. Repeated detection updates that proposal instead of creating duplicates.
New items continue importing even when other items require review. Missing source
items leave both the live record and any pending review unchanged. The original
strict `legacy_import.py import` command still aborts on changed identities.
Use `legacy_import.py sync` for the same additive import from an existing bundle.

Deploy the updated image and start the updated application first to apply migration
`0008_legacy_sync_reviews.sql`. After reviewing the target database and its backup,
run once on the host where production Compose is already running:

```sh
docker compose --env-file deploy/.env -f compose.production.yaml exec -T web \
  python3 scripts/legacy_sync.py run \
  --state /app/migration \
  --page-map config/legacy-vysker-pages.json \
  --notice-map config/legacy-vysker-notices.json \
  --publish-content
```

The updated web image contains the Python dependencies, scripts and maps. The
private `migration-state` volume stores reports, persistent cache and temporary captures, outside
the public website. Database access uses the existing `OBEC_DATABAZE_FILE` secret.
The crawler rechecks source pages and downloads new attachments. Choose the interval
with the source's request rate and available bandwidth in mind. Completed runs
remove temporary captures. Cache objects are shared with temporary bundles through
hardlinks when supported. Allow disk space for one copy of the cached attachment
library in addition to the database. The cache can be rebuilt and is not a backup.
Changed source bytes are retained in PostgreSQL with their proposal and are
included in normal database backups. Private exported review files may be removed
and regenerated from the database.

`deploy/legacy-sync.cron.example` is an hourly `/etc/cron.d/` example. Replace
`/srv/vysker` with the deployment checkout and arrange rotation of
`/var/log/vysker-legacy-sync.log`. Installing the cron entry is a separate server
operation. The job returns a nonzero exit code on failure, writes a compact result
to stdout on success, and keeps these private files in `/app/migration`:

- `last-run.json`: running, completed or failed attempt, including whether import committed
- `last-success.json`: last completed synchronization, item counts/lists,
  `duration_seconds` and `download_stats` (HTTP requests, downloaded bytes,
  downloaded/reused attachments, database-seeded files and unchanged HTML responses)
- `capture-errors.json`: failed source requests, including any deliberately allowed subset
- `capture-unavailable.json`: HTTP 404 source resources observed in the latest capture,
  retained even if another capture error prevents import
- `reviews.html` and `reviews.json`: all pending proposals with existing destinations,
  current local text, source text/metadata and downloadable changed attachments
- `capture-cache/`: private reusable response metadata and verified source bytes

Monitor the age of `last-success.json`, failures and `pending_reviews`. Unchanged
source items do not create new reviews. No subscriber emails are sent. By default,
new content is private. The example's `--publish-content` publishes new ordinary
pages, documents and events, while notices remain drafts for editorial review.
Add `--archive-notices` to `legacy_sync.py run` to put new historical notices
directly into the public archive instead.
Previously imported drafts keep their existing publication state.

Incomplete captures fail without database changes by default. Only after reviewing
`capture-errors.json`, an operator may add `--allow-incomplete` to import the
successfully captured subset. Missing pages are never treated as deletion requests.
A failed report export after a committed import also returns failure, and retrying
still cannot duplicate the already imported records.

Download the private review report to the operator's computer, including its
linked `review-files` directory:

```sh
docker compose --env-file deploy/.env -f compose.production.yaml cp web:/app/migration/. data/legacy-sync-review/
```

Read `reviews.html`, make any accepted changes through the administration, then
mark the exact proposal reviewed using its `source_key` and `fingerprint` from
`reviews.json`. This confirmation does not apply content changes:

```sh
docker compose --env-file deploy/.env -f compose.production.yaml exec -T web \
  python3 scripts/legacy_sync.py acknowledge \
  --source-key 'ms:1053' --fingerprint 'FINGERPRINT_FROM_REVIEW'

docker compose --env-file deploy/.env -f compose.production.yaml exec -T web \
  python3 scripts/legacy_sync.py reviews --output /app/migration
```

A stale confirmation is refused if another source change has replaced the
proposal. The same acknowledged change stays closed on later runs. A different
change reopens the review. Stop the cron entry before switching `vysker.cz` to
the new application, after the final synchronization and content review.

## Mapping and fidelity

The optional page map assigns the original office contact, municipality section
and mandatory-information section to `kontakt`, `obec` and `povinne-informace`.
Other pages get stable generated slugs. Existing slug collisions abort the import.
Nothing in the map approves the accuracy of the original content or replaces
the new website's accessibility statement.

HTML is converted into restricted Markdown. Headings, paragraphs, lists, links,
tables as text rows and descriptions remain editable. JavaScript, forms, shared
template controls and remote embeds are removed. Imported verified raster images
can be shown inline through `/api/v1/legacy-media/{id}`. Other files use the normal
attachment download path. Original HTML remains available for comparison.

Photo detail URLs (`g:`), including slideshow and ASP variants, are aliases of
their owning article or gallery, not separate content pages. The importer uses
explicit gallery backlinks and embedded gallery lists to establish ownership.
It replaces thumbnails with verified full-size photographs, preserves captions
and article text, and includes each photograph only once. An embedded gallery
(`gs:`) also redirects to its article. A standalone gallery remains one page.
Missing or ambiguous ownership is deferred for review instead of creating photo
pages. `grouped_gallery_pages` lists the source identities and their owners.

Grouped content participates in change detection, so photos added through a
detail page are proposed for review during sync without overwriting an edited
article. Original photo addresses retain their own provenance and redirect to
the article while it is public. Image files continue using the existing attachment
storage and editor image library.

Calendar entries are converted to structured events when their dates can be
parsed unambiguously. Missing clock times and end dates are explicitly marked as unknown.
Internal day boundaries are used only to sort and filter such events. Public
labels say `původní web čas neuváděl` or `Původní web datum konce neuváděl`.
Unparseable or ambiguous dates remain
in content pages and are flagged for review. Editing a stored event time makes
that specific time a known value. Editing other fields preserves its precision.

Original notice dates remain source facts. Import never supplies a guessed
publication timestamp, applies a new default 15-day period, generates historical
publication evidence or automatically republishes a notice. Notice attachments
stay attached to drafts. Review active notices and their actual publication
continuity separately before the domain switch. Archived records and decisions
about continued attachment availability need municipal review.

Each imported record has immutable provenance in `legacy_sources` and an audit
entry. Page imports also create the first page revision. The original title,
capture metadata, source dates and file checksum remain available to operators.

## Navigation ownership and original dates

With `--classify-navigation`, the importer reads the `.cesta` breadcrumb from
captured HTML. Every attachment belonging to a page whose breadcrumb contains
`Úřední deska` is imported exclusively as a notice, including Dotace, forms and
undated files. Other attachments become ordinary documents. A link to the board
in the common menu does not count. If a file has several source parents and any
of them belongs to the board, it has one notice owner and one stored attachment.
`config/legacy-vysker-notices.json` supplies categories only, not ownership.

Original posting and withdrawal dates stay nullable. Labels such as `Vyvěšeno`
and `Sejmuto` are parsed from the text beside each source attachment. Missing or
incomplete dates are not derived from the capture timestamp, page modification,
filename or a default 15-day period. A conflicting withdrawal date is retained in
source metadata for review, but not used as a known date. Public and admin views
say `Datum vyvěšení není uvedeno` and `Datum sejmutí není uvedeno` as appropriate.
Both lists sort newest first, with undated records last in either date direction.

Public document and notice details show a separate import-origin note based on
the saved provenance, including archived notices. The note contains no link to
the original source. Existing imports gain this note without another import.

`--preview-notices` makes notices visible for a local manual rehearsal only. It
requires navigation classification, non-production configuration and loopback
website/database addresses. A notice missing either its posting or withdrawal
date goes to the archive, even if its known posting date is in the future.
The missing fields are recorded in import metadata. Notices with both dates
known are archived after expiry, published during their posting period, and
kept as drafts before that period. Source `Lhůta do` stays in source metadata
and is **never stored as a withdrawal date** or used to establish publication.

Preview attachments remain available for testing. Import creates neither actual
publication/withdrawal timestamps, publication events nor notification messages.
Production startup rejects databases containing these preview records.

`--archive-notices` is a separate mode for importing historical notices directly
into the public archive. It applies to both structured notices and files
classified under the original notice-board navigation. These records never become
active notices, even when a source date is missing or lies in the future.
Their original titles and attachments remain available, with archive retention
set to `9999-12-31`. The operator can subsequently change the retention policy.
Original dates are preserved without manufacturing publication or withdrawal
timestamps. The import does not create publication evidence or send notifications.
This mode cannot be combined with `--preview-notices`. Without either flag,
notices remain private drafts. The staging systemd service selects archive mode.

### Archive existing imported notices

Enabling `--archive-notices` affects newly imported notices. To convert previously
imported drafts, first inspect the read-only plan against the intended database:

```sh
python3 scripts/legacy_archive.py plan --report /app/migration/archive-plan.json
```

Then apply the reviewed candidate count, replacing `826` with the plan's count:

```sh
python3 scripts/legacy_archive.py apply --expected-count 826 \
  --report /app/migration/archive-result.json
```

The conversion selects untouched imported drafts through their provenance and
updates archive status, title retention and attachment retention in one
transaction under the application's write lock. Native drafts, scheduled notices,
edited imports, pages, documents and calendar entries are left unchanged.
Existing IDs, original dates and attachment bytes are preserved. Each converted
notice gets an audit entry, without a fabricated publication or withdrawal event
and without subscriber mail. Repeating the plan after conversion finds no
remaining candidates.

The older `legacy_reconcile.py` workflow made copies of already imported files.
It is retained for older rehearsals, but is not used for the exclusive import.
An existing library with different ownership is reported as a conflict instead
of silently changing its records.

### Replace imported staging content

For an explicitly requested fresh import of documents, archived notices and
calendar entries, `legacy_replace.py` replaces only records identified by import
provenance. Existing imported pages and galleries are removed. Native content,
accounts, subscriptions, configuration and audit history remain in place.
Stop the synchronization timer while preparing the replacement.

First capture a fresh bundle with `legacy.py capture`. The replacement validates
every captured object before changing the database. Then run against the intended
staging database, using the inspected current `legacy_sources` count:

```sh
OBEC_PRODUCTION=false python3 scripts/legacy_replace.py \
  --bundle /app/migration/fresh-capture \
  --report /app/migration/replacement.json \
  --expected-source-count 1515 \
  --notice-map config/legacy-vysker-notices.json \
  --archive-notices --skip-pages
```

This staging-only operation intentionally creates no database backup. It removes
and reimports content in one transaction, so a validation or import failure
restores the previous state. Local edits to imported records are replaced too.
Record sequences are not reset, and attachment IDs are retained by source identity
where the same file remains in scope. Original source dates remain unchanged.
No subscriber email is sent. Verify the public archive and attachment downloads,
then re-enable the timer with `--archive-notices --skip-pages` selected.

## Clean local reimport

Only when intentionally replacing **all** local documents and notice records,
stop the local application and run:

```sh
python3 scripts/legacy_reimport.py \
  --bundle data/migration/vysker-snapshot \
  --notice-map config/legacy-vysker-notices.json \
  --output data/migration/clean-rehearsal
```

The output directory must be new. The command validates all source objects,
creates and checks a full PostgreSQL dump, then deletes the library and imports
its replacement in one transaction. `--allow-incomplete` is available after
reviewing capture errors. A failure rolls the reset and import back together.
The local-only guard also applies to the reset. Protected deletion triggers are
restored in the same transaction, and the reset is recorded in the audit log.

Pages, revisions, navigation, calendar, administrator accounts, subscribers and
audit history are preserved. Existing attachment IDs are reused by source identity
so links already embedded in pages and calendar entries remain valid. Document
and notice IDs are not reset or reused. Local edits to documents/notices and
non-imported library entries are intentionally removed, and recoverable from the
backup. Start the updated app before testing the replacement library.

To replace **all previously imported content**, including pages, their revisions
and uploaded images, calendar entries and pending sync proposals, add
`--all-imported`. This mode selects records through import provenance and retains
unrelated local pages, documents, notices and events. Local edits to imported
records are removed with those records and remain recoverable from the backup.
Pass `--page-map config/legacy-vysker-pages.json` to retain the reviewed page slugs.
The same source validation, backup, local-only guard and atomic rollback apply.
The updated import omits pages linking to documents and imports their files.

## Historical URLs

The server resolves Vismo entity IDs, friendly URLs, encoded hyphens and supported
ASP query URLs to imported content using HTTP 301. Query parameters identifying
assets remain significant. Only currently public targets redirect. Downloads and
images still enforce publication and withdrawal checks. Unresolved internal links
and old fragments are listed in the report for reconciliation.

Test redirects on the new host before switching DNS. The old domain must point
to the new application for these redirects to handle visitors' old bookmarks.

With the preview running and `OBEC_DATABAZE` pointing to that same database:

```sh
python3 scripts/legacy_verify.py --base-url http://127.0.0.1:3012 \
  --report data/migration/verification.json
```

This read-only check requests every imported historical path, compares the HTTP
redirect with its current publication state, downloads every public imported
attachment from either section and compares its size and SHA-256 with the database. It also
reports record counts and the mail queue size. It exits unsuccessfully on a
redirect or byte mismatch. It does not send email or edit content.

## Acceptance and final switch

1. Compare the capture inventory with the sitemap, menus and notice-board archive.
2. Resolve failed downloads and review empty pages, dates and unmapped links.
3. Open important pages and documents, compare text, tables, photographs and dates.
4. Verify downloads, content hashes, redirects, page editing and calendar precision.
5. Repeat the import to confirm zero duplicates and zero notification messages.
6. Perform a fresh capture before cutover and reconcile changes since rehearsal.
7. Agree notice-board continuity, approve content and switch DNS only after acceptance.

Website export does not contain private subscriber lists, administrator accounts
or original-system audit records. Those cannot be reconstructed from public pages.

## Moving this application to another host later

The application is portable as a Docker image plus PostgreSQL and configuration.
Files are stored in PostgreSQL, so the database dump includes attachments.
Preserve the offline migration bundle separately if source HTML and capture
responses are required for later review. Those originals are not stored in the
application database.

For a planned transfer, stop **all** old application instances, mail workers and
writers before the final `pg_dump`. Restore the snapshot on the new host with a
compatible PostgreSQL version, set database and SMTP secrets, configure HTTPS and
administration access, then start only the new instance and switch DNS. Keep the
old instance stopped to avoid independent writes or duplicate mail deliveries.

Do not use the disaster-recovery `scripts/database.py restore` command for this
planned transfer. That command deliberately discards subscriptions and tokens
because an older backup may contain withdrawn consent. A current, quiesced
planned-transfer snapshot preserves subscribers, consent and the mail queue.
Verify counts, hashes and actual downloads before retiring the old database.

In Compose, database data lives in `postgres-data`, outside the container's
writable layer. Host and container restarts preserve it. Deleting the volume,
including `docker compose down -v`, destroys it. Local backup volumes share the
host's failure domain, so independent offsite backups are still required.
