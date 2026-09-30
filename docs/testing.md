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
psycopg and PostgreSQL 18 client tools, all included in Nix and the test image.

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
shutdown. Do not point the test suite at a deployed website.

Covered workflows:

- Keyboard skip link, pagination and Czech search without diacritics
- Creating a notice, uploading a file, publishing and anonymous download
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
