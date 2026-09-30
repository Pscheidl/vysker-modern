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
  --report data/migration/plan.json

python3 scripts/legacy_import.py import \
  --bundle data/migration/vysker-snapshot \
  --page-map config/legacy-vysker-pages.json \
  --report data/migration/import.json
```

Planning runs in a read-only transaction. Import validates all referenced objects
and writes one database transaction under the application's write lock. It creates
drafts by default. A repeated import skips unchanged identities. Changed source
records cause a conflict before any writes, including when an editor has already
changed the imported record. The importer never overwrites local edits.

`--publish-content` publishes ordinary pages, documents and calendar entries for
a reviewed test environment. Notices remain drafts. This option does not send
subscription notifications. It also does not publish previously imported drafts
on a repeated run. Use the administration for subsequent editorial decisions.

Incomplete captures are refused by default. After reading the capture errors,
`--allow-incomplete` allows the successfully captured subset to be imported.
Failures remain in the import report. This flag is **not** a claim of complete
coverage, and missing documents must be reconciled before launch.

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
document attachment and compares its size and SHA-256 with the database. It also
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
