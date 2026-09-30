# Vyskeř backend

The backend uses Axum 0.8, SQLx 0.9, PostgreSQL, Argon2id and lettre. The same API
runs standalone or alongside Leptos SSR. The public GitHub Pages preview is a
separate client build with sample data and no database access.

## Local development with Nix

Enable the `nix-command` and `flakes` features in Nix. Dependencies are pinned
in `flake.lock`, and Rust 1.98.1 is pinned in `rust-toolchain.toml`.

```bash
nix develop
cp .env.example .env
mailpit --listen 127.0.0.1:8025 --smtp 127.0.0.1:1025
```

Keep Mailpit running in the first terminal. In a second terminal:

```bash
nix develop
./scripts/backend.sh
```

API health endpoint: http://localhost:3000/api/v1/health.
Captured email: http://localhost:8025. Mailpit does not deliver messages externally.

For the full website, run `./scripts/dev.sh` instead of `backend.sh`. Both
scripts load the local, ignored `.env` file. Both modes use the same default
port, so run one at a time. Open the exact origin configured in `OBEC_VEREJNA_URL`.

Without Nix, install the Rust toolchain from `rust-toolchain.toml` and use
Mailpit or Compose for SMTP tests. The backup test also requires Python 3.

## Development Docker Compose

```bash
docker compose up --build -d
docker compose exec api ./obec-admin admin@example.test
```

The administrator command prompts for a password without displaying it.
There is no default account or password. You can also create an account locally:

```bash
cargo run --locked --bin obec-admin -- admin@example.test
```

The CLI reads environment variables. If the server uses a custom database path,
pass the same `OBEC_DATABAZE` value to the CLI. Unlike the development scripts,
the CLI does not load `.env` itself. For automation, use `--password-stdin` to
read the password from standard input.

Development Compose runs the **backend API and Mailpit**. Run the full Leptos
frontend separately through `dev.sh`. PostgreSQL uses the named `postgres-data` volume.
The test service uses isolated test databases, never this application volume.
`docker compose down` preserves application data.

The complete website deployment is described in [operations.md](operations.md).

## Tests

```bash
# Unit, API, migration, privacy and backup tests without external services
./scripts/test.sh

# The same tests plus SMTP delivery to a running Mailpit instance
./scripts/test.sh --smtp

# The complete test environment in Compose
docker compose --profile test run --build --rm tests
```

You can also use `nix develop -c ./scripts/test.sh`. The SMTP test reads
`TEST_SMTP_HOST` and `TEST_SMTP_PORT`, defaulting to `127.0.0.1:1025`.

Tests cover authentication, CSRF, document states, the default 15-day interval,
minimum publication dates, permanent archive records, attachment removal and
retention, missed schedules, Europe/Prague dates, repeated maintenance, legacy
migrations, subscription verification and withdrawal, the mail queue, audit
protection, privacy retention, proxy trust, readiness and database recovery.

The `.github/workflows/backend.yml` workflow runs the Compose tests. A second
job validates production Compose and Caddy, builds the full Leptos image and
checks production startup and JS/WASM/CSS delivery. Development and test profiles
limit debug information to avoid DWARF limits when linking generic Leptos types.

## Authentication and administration

### Administration screens

The full application started through `./scripts/dev.sh` provides `/admin`.
Sign in using an account created with `obec-admin`. The standalone Compose
`api` service does not serve these screens.

- The dashboard shows notice, draft, document, page and subscriber counts.
- Notice and document lists support title search, state filtering and pagination.
- Save a new record as a draft, upload attachments, then publish it.
- Notices support an interval, a specific withdrawal date or no end date.
- Publication requires selecting the applicable rule and reviewing the content.
- Withdrawal dialogs explain what will happen to attachments.
- Draft attachments can be downloaded and removed. Published attachments are immutable.
- Content pages have a plain text editor, a preview, a slug and a publication switch.
- Audit history is read-only and identifies an administrator or system action.
- `/admin/posta` lists recipients and SMTP acceptance status without message bodies.
- Publication evidence can be downloaded, and availability incidents can be recorded.

Administration supports both themes and smaller screens. It warns about unsaved
changes when leaving through its navigation, signing out or closing the tab.
After session expiry, an error offers a login link that opens in a new window.
Sign in there, then retry the original operation. Writes fetch a fresh CSRF token.
Passwords and session cookies are not stored in localStorage.

GitHub Pages hosts the public preview only, with no administration service.

### API login example

```bash
curl -sS -c /tmp/vysker-cookies -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.test","password":"YOUR_PASSWORD"}' \
  http://localhost:3000/api/v1/admin/login
```

This illustrates the JSON contract. In a real client, read the password from a
secure input instead of placing it in shell history.

The response includes `administrator_id` and `csrf_token`. The server sets an
`obec_session` cookie with HttpOnly, SameSite=Strict and an eight-hour lifetime.
HTTPS adds Secure. Authenticated writes require this cookie and `X-CSRF-Token`.
Retrieve the current token with `GET /api/v1/admin/session`.

```bash
curl -b /tmp/vysker-cookies -H 'Content-Type: application/json' \
  -H 'X-CSRF-Token: TOKEN_FROM_LOGIN' \
  -d '{"title":"Municipal notice","description":"Notice text"}' \
  http://localhost:3000/api/v1/admin/notices
```

A `201 {"id":1}` response identifies a new draft. Before publication, upload an attachment:

```bash
curl -b /tmp/vysker-cookies -H 'X-CSRF-Token: TOKEN_FROM_LOGIN' \
  -F 'file=@notice.pdf' \
  http://localhost:3000/api/v1/admin/notices/1/attachments

curl -X POST -b /tmp/vysker-cookies -H 'X-CSRF-Token: TOKEN_FROM_LOGIN' \
  http://localhost:3000/api/v1/admin/notices/1/publish
```

Production requires the review fields described below before publication.

### API reference

Every path in this table begins with `/api/v1`. Paginated lists accept `limit`
from 1 to 100 and an `offset`. `PUT` takes the complete editable state rather
than a partial patch.

| Method and path | Purpose |
| --- | --- |
| `GET /health` | API and database health |
| `GET /ready` | Database health and maintenance heartbeat |
| `GET /categories` | Notice board categories |
| `GET /notices?archived=false` | Public board, or archive with `archived=true` |
| `GET /notices/{id}` | Notice including attachment metadata |
| `GET /documents`, `GET /documents/{id}` | Published general documents |
| `GET /attachments/{id}` | Download an available attachment |
| `GET /pages/{slug}` | Published content page |
| `GET /privacy` | Current subscription information and fingerprint |
| `POST /subscriptions` | Opt-in with `email` and `consent.fingerprint`, see [privacy.md](privacy.md) |
| `GET /admin/subscribers/{id}/consents` | Read-only opt-in and opt-out evidence |
| `POST /subscriptions/verify`, `POST /subscriptions/unsubscribe` | JSON `token` |
| `POST /admin/login` | JSON `email` and `password` |
| `GET /admin/session`, `DELETE /admin/session` | Current session and logout |
| `GET /admin/audit` | Read-only audit history, newest first |
| `GET /admin/mail` | Read-only mail history without bodies or tokens |
| `GET /admin/overview` | Counts and the current date in Prague |
| `GET /admin/notices/{id}` | Notice details including drafts and attachments |
| `GET /admin/notices/{id}/evidence` | Publication evidence export |
| `POST /admin/notices/{id}/incidents` | Record an availability incident |
| `GET /admin/documents/{id}` | General document details including drafts |
| `GET /admin/pages/{id}` | Content page details including drafts |
| `GET /admin/attachments/{id}` | Authenticated download, including drafts |
| `DELETE /admin/attachments/{id}` | Remove draft file contents while retaining metadata |
| `GET /admin/notices`, `POST /admin/notices` | List notices or create a draft |
| `PUT /admin/notices/{id}` | Edit a draft and its publication rules |
| `POST /admin/notices/{id}/publish` | Publish or schedule a notice |
| `POST /admin/notices/{id}/withdraw` | Withdraw a notice into the archive |
| `POST /admin/notices/{id}/attachments` | Upload one multipart `file` |
| `GET /admin/documents`, `POST /admin/documents` | Manage general documents |
| `PUT /admin/documents/{id}` | Edit a document draft |
| `POST /admin/documents/{id}/publish` | Publish and notify subscribers |
| `POST /admin/documents/{id}/archive` | Hide a general document |
| `POST /admin/documents/{id}/attachments` | Upload a document attachment |
| `GET /admin/pages`, `POST /admin/pages` | List or create content pages |
| `PUT /admin/pages/{id}` | Edit a page and its publication state |

Administration lists for notices, documents and pages also accept `q` for title
search and `status` for filtering. Search wildcards are treated literally.
These queries use PostgreSQL LIKE and do not normalize Czech diacritics. Public
browser search normalizes diacritics separately.

### Official notice board

Draft input includes `title`, `description`, `reference_number`, `category_id`,
`issuer`, `published_on`, `duration_days`, `withdraw_on`, `unlimited`,
`retain_attachments` and a `review` object matching `NoticeReview` in
`src/notice_policy.rs`.

- The default publication date is today in Europe/Prague.
- The default interval is 15 days. `duration_days` accepts 1 to 3650, subject
  to the minimum for the chosen publication rule.
- `withdraw_on` specifies a date instead of an interval.
- `unlimited=true` disables automatic withdrawal and cannot be combined with an interval.
- `retain_attachments` defaults to `false`.
- The withdrawal date is the first day the notice is no longer displayed.
  Dates use `YYYY-MM-DD`.
- Withdrawal always preserves a minimal archive record and attachment metadata.
- Without archive retention, file contents are removed in the same transaction.
- Only drafts can be edited. Published, scheduled and archived notices are immutable.
  Correcting published content requires a new notice.

The maintenance task runs on startup and on a 30-second schedule. Public queries
apply visibility rules immediately at expiry, even before cleanup runs. A missed
scheduled publication is returned to draft if its interval is still valid, or
archived without a notification if the entire interval has expired. See
[notice-board.md](notice-board.md) for review rules, evidence and incident handling.

### General documents and content pages

A document accepts `title` and `description`. Edit and attach files while it is
a draft, then publish. Archived general documents are hidden from public access.
Public document URLs are `/dokumenty/{id}`.

A page accepts `slug`, `title`, `content` and `published`. Its generic public URL
is `/stranky/{slug}`. Content is escaped plain text with preserved line breaks.
The editor includes a preview. Published pages appear under `/stranky`, linked
from the footer. Rich text, revisions and configurable navigation remain future work.

The slugs `kontakt`, `obec`, `kalendar`, `pristupnost` and `povinne-informace`
also populate their dedicated public routes. Production requires these pages to
be published and prevents hiding or renaming them. Their contents remain editable.

### Attachments

The per-file limit is 10 MiB. Supported types are PDF, PNG, JPEG, TXT, CSV, DOCX,
XLSX, ODT and ODS. Validation checks extensions and basic signatures, and requires
UTF-8 for text. Downloads use attachment disposition, `nosniff` and a separate
route. Signature checks are not antivirus scanning.

Contents, SHA-256 hashes and metadata are stored in PostgreSQL. Upload and removal
are atomic with the associated domain operation and audit. Larger deployments
could move contents to object storage. Legacy sample attachment names have no
file contents and cannot be downloaded.

### Subscriptions and mail

Verification links are single-use and expire after 24 hours. Unverified
addresses receive no publication notifications. Verified subscribers receive
future notices and general documents. Republishing the same record does not
enqueue duplicate notifications.

Links open confirmation forms. GET requests do not change subscription state,
preventing automated email scanners from activating or cancelling subscriptions.
New notification emails contain unsubscribe links without a fixed expiry date,
usable while the associated subscription remains active.

The durable mail queue is populated in the publication transaction. SMTP
failures use increasing retry delays, capped at six hours. Message bodies are
removed after SMTP acceptance or cancellation. Worker leases expire after a
crash. Delivery is at least once. A crash after SMTP acceptance but before the
database update can cause another delivery with the same Message-ID.

Mail history records SMTP acceptance rather than final inbox delivery or reading.
Bounce processing, complaint feedback and per-attempt SMTP outcomes are not implemented.

### Audit and authentication

Domain writes and audit entries are transactional. Triggers reject updates to
audit rows and deletion of notice records. Audit deletion is permitted only by
controlled cleanup after the approved retention period. Passwords use Argon2id.
Authentication and subscription token tables store SHA-256 hashes only. The mail
queue temporarily contains complete email bodies including links, then removes
them after sending or cancellation.

Audit entries contain no passwords or tokens. Login and verification requests
are rate limited. Forwarded IP addresses are accepted only through `X-Real-IP`
from the exact peer set by `OBEC_TRUSTED_PROXY`. `X-Forwarded-For` is not trusted.

## Configuration

| Variable | Default or purpose |
| --- | --- |
| `OBEC_ADRESA` | `127.0.0.1:3000` |
| `OBEC_MIN_PASSWORD_LENGTH` | `24` characters |
| `OBEC_DATABAZE_FILE` | Optional secret file, mutually exclusive with `OBEC_DATABAZE` |
| `OBEC_DATABAZE` | `postgresql://vysker:vysker@127.0.0.1:5432/vysker` |
| `OBEC_VEREJNA_URL` | `http://localhost:3000`, without a subpath |
| `OBEC_UKAZKOVA_DATA` | `false` |
| `OBEC_PRODUCTION` | `false`, production enables HTTPS, SMTP, privacy and content checks |
| `OBEC_PRIVACY_CONFIG` | JSON policy path, subscriptions are disabled when unset |
| `OBEC_SMTP_HOST` | `localhost` |
| `OBEC_SMTP_PORT` | `587` |
| `OBEC_SMTP_TLS` | `starttls`, accepts `starttls`, `tls` or `none` |
| `OBEC_SMTP_UZIVATEL`, `OBEC_SMTP_HESLO` | Unset |
| `OBEC_SMTP_HESLO_FILE` | Optional password file instead of `OBEC_SMTP_HESLO` |
| `OBEC_TRUSTED_PROXY` | Optional exact trusted proxy IP address |
| `OBEC_EMAIL_OD` | `Vyskeř <noreply@localhost>` |

Configure the public HTTPS origin and a real SMTP service for production.
If switching from localhost to 127.0.0.1, update `OBEC_VEREJNA_URL` too. It is
used for email links and administrative origin checks.

The homepage fetches only three active and three archived notices, with a separate
active count. Public notice and document lists and site-wide search use server
pagination with 20 results per page. A notice detail loads directly by ID.
`GET /api/v1/search` accepts `q`, `kind` (`all`, `notice`, `document`, `page`),
`state` (`current`, `archive`, `all`), `category`, `sort` (`newest`, `oldest`,
`title`) and a one-based `page`. Empty filters use current records and newest
first. Queries accept at most 200 characters and 10 whitespace-separated words.
Search ignores Czech diacritics and case, treats percent and underscore literally,
and requires every word to match. The query and result count both use the public
archive projection, never withdrawn descriptions, issuers or reference numbers.

Search currently scans public text in PostgreSQL. It removes the old 1,000-record
frontend limit, but it is not a full-text index. Measure the production data set
before adding a larger search index. The published page index still has a
1,000-page display limit, while site-wide search can find pages beyond it.

## Technical references

- [SQLx transactions](https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html)
- [Axum Multipart and request size](https://docs.rs/axum/latest/axum/extract/struct.Multipart.html)
- [Lettre SMTP](https://docs.rs/lettre/latest/lettre/)
- [Rust overlay for Nix](https://github.com/oxalica/rust-overlay)
- [Mailpit in Docker](https://mailpit.axllent.org/docs/install/docker/)

## Schema and operations

See [database.md](database.md) for the English schema, migration precautions and
PostgreSQL implementation. The application now uses PostgreSQL exclusively.

`GET /api/v1/ready` checks the database and maintenance heartbeat.
`GET /api/v1/admin/mail?limit=20&offset=0` returns read-only message history.
See [operations.md](operations.md) for deployment, backup, restore and monitoring.

## Administrator accounts

Open `/admin/ucty` to change your password, create an administrator, activate or
deactivate an account, or revoke all of its sessions. Every account mutation
requires your current password in addition to the session cookie and CSRF token.
All administrators have equal privileges. There is no editorial role hierarchy.
The final active administrator cannot be deactivated.

Password changes, deactivation and explicit session revocation invalidate all
sessions for the affected account. Password changes require a fresh login.
Authentication is rechecked inside the write transaction so concurrent password
changes or deactivation cannot be bypassed by a login already in progress.
Reauthentication is limited to ten attempts per account per fifteen minutes.

For forgotten passwords, the operator runs the following with the same database
and application configuration as the server. The password prompt is hidden.

```sh
cargo run --locked --bin obec-admin -- reset-password admin@example.cz
```

In production, run `/app/obec-admin reset-password admin@example.cz` inside the
web container. `--password-stdin` is available for a secure input pipe. Do not put
passwords in command arguments, shell history, or tracked configuration.
Recovery revokes sessions and records `password_reset_by_operator` in the audit.
It does not activate a disabled account. Another active administrator can do that
in the UI. Existing `obec-admin EMAIL` usage still creates a new account, and
`obec-admin create EMAIL` is an explicit equivalent.
