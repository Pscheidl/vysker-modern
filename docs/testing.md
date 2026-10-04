# Testing

## Rust, API, SMTP and operational scripts

```sh
./scripts/test.sh
# With Mailpit listening on localhost:1025
./scripts/test.sh --smtp
```

The suite includes account authorization, password/session lifecycle, archive
privacy, pagination beyond 1,000 records, migrations, retention, mail handling,
backup and restore, and monitoring thresholds and notification deduplication.
Some integration tests bind local sockets, so run outside a sandbox that denies
loopback listeners. Rust API tests use separate schemas in a real PostgreSQL
database. The test script creates a temporary local PostgreSQL cluster by default,
then removes it on exit. Alternatively, supply `TEST_DATABASE_URL` for a test
server account with `CREATEDB`. The script creates and removes a random database
on that server, without using its existing application data. Python tests require
psycopg, Beautiful Soup, timezone data and PostgreSQL 18 client tools, all included
in Nix and the test image.

## Browser regressions

Install Node.js 22 or newer, pnpm, Python 3, Rust and cargo-leptos. The Nix shell
includes these development tools. Build the SSR application and WASM assets, then
run the pinned Playwright suite with the local PostgreSQL instance available:

```sh
cargo leptos build --lib-cargo-args=--locked --bin-cargo-args=--locked
cargo build --locked --bin obec-admin
pnpm install --frozen-lockfile
pnpm exec playwright install --with-deps chromium
TEST_DATABASE_URL=postgresql://vysker:vysker@127.0.0.1:5432/postgres pnpm test:e2e
```

The runner starts `scripts/e2e-server.py` on `127.0.0.1:3107` and refuses to reuse
an existing server. It creates its own temporary PostgreSQL database, synthetic
administrator, privacy configuration and 25 notice records. It does not read
`.env`, reuse the developer database, or send real email. Test links are read from
the fixture's local mail queue. The server and temporary database are removed on
shutdown. Shared fixtures reset rate-limit counters in that disposable database
before each test, so repeated administrator logins in unrelated scenarios do not
exhaust one another's limits. Throttling remains active within each test.
Import `test` from `tests/browser/fixtures.ts` in every browser spec.
Do not point the test suite at a deployed website.

Covered workflows:

- Keyboard skip link, pagination and Czech search without diacritics
- Creating a notice, adding multiple attachments, preserving them after reload,
  draft privacy, publishing and anonymous download of every attachment
- Subscription request, confirmation, notification and unsubscribe
- Account settings and viewport overflow on mobile

The Rust account tests cover password changes, operator recovery, session
revocation and final-administrator protection. Browser tests do not alter the
local developer administrator's credentials.

HTML reports and failure traces are written under `target/playwright-report` and
`target/playwright-results`. They contain only synthetic test data. To view a
report, run `pnpm exec playwright show-report target/playwright-report`.

For a containerized run with the same production binary used in deployment:

```sh
docker build -f Dockerfile.web -t vysker-web:test .
docker build -f Dockerfile.e2e -t vysker-e2e:test .
docker compose up -d postgres
# Linux host networking reaches the PostgreSQL port bound to loopback.
docker run --rm --ipc=host --network host \
  -e TEST_DATABASE_URL=postgresql://vysker:vysker@127.0.0.1:5432/postgres vysker-e2e:test
```

The backend GitHub workflow builds the production image, runs its smoke test,
then runs the browser suite and uploads the HTML report. The `tests` job also
runs the Rust and Python tests, including the Mailpit check.

Automated checks do not replace manual accessibility assessment, review of actual
municipal documents, an operator alert delivery drill, or offsite restore testing.


## Editorial regression coverage

The Rust tests cover single-use email recovery, expiration, revocation, salted
passwords, configured minimum length, safe Markdown, page revisions, concurrent
edit conflicts, navigation validation and category preservation. Calendar tests
cover publication, cancellation, Prague time and DST validation. Subscriber tests
cover authorization, search, evidence export, withdrawal, erasure and SMTP leases.

Browser scenarios exercise recovery, Markdown preview, conflicts between tabs,
restoring revisions, menu changes, calendar publication and subscriber workflows.
Page image tests cover decoded file formats and limits, upload authorization,
draft privacy, publication and withdrawal of images, preserved revisions, Unicode
toolbar selection, failed upload recovery, unsaved edits and mobile layout.
Imported image regressions cover page-specific libraries, reference deduplication,
images preserved in older revisions, private draft previews, reuse of public URLs
and exclusion of removed files or Markdown code examples.
Run Rust tests before building the full web for browser tests. `cargo test --bins`
can replace the SSR binary with a build lacking cargo-leptos's compile-time asset
settings. Rebuild with `cargo leptos build` before testing hydration. Do not run
Cargo builds that share the same target directory concurrently with browser tests.

## Legacy migration coverage

`tests/test_legacy.py` uses synthetic offline bundles and disposable PostgreSQL
databases. It checks source identities, extraction, attachment hashes, unsafe
paths, original dates, missing event times, DST ambiguity, draft defaults,
incomplete-capture rejection, idempotence, preserved editor changes, notice
attachment privacy, page revisions and the absence of notification messages.
`tests/legacy.rs` covers publication-aware historical redirects, local image
rendering and access revocation, and the explanation for missing original times.

For an actual captured website, `scripts/legacy_verify.py` checks every imported
historical redirect and downloads every public imported attachment to compare
its checksum and size with PostgreSQL. Run it only against the intended preview
and its database. See [migration.md](migration.md) for commands and limits.
Real municipal data stays under ignored `data/` and is not uploaded as a CI artifact.
