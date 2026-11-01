# Vyskeř municipal website

A municipal website for Vyskeř, Czech Republic. The public frontend uses
**Leptos 0.8 + Axum 0.8**, with server rendering and Rust interactivity through
WebAssembly.

Selected design: [landscape relief, light and dark themes](design/01-relief/README.md).
Current illustration and identity: [Saint Anne's Chapel and the treeless Hůra hill](design/02-vysker-relief/README.md).

## Architecture

- Frontend: **Leptos**, server rendering and Rust WebAssembly hydration.
- Backend: **Axum**, **SQLx 0.9** and **PostgreSQL 18**.
- The approved visual concept is implemented as a responsive website.
- The former Askama frontend and earlier static prototypes have been replaced.
- Database identifiers, API fields and project documentation use English.
  The municipal website and its public routes use Czech.

## Implemented pages and controls

- Homepage with a landscape relief, shortcuts and recent documents.
- Official notice board, archive and document details backed by PostgreSQL.
- Category filters, sorting and browser search that ignores Czech diacritics.
- General documents and forms, search, municipal information and contact details.
- Structured calendar with a homepage feed, event details and archive, subscriptions, accessibility, privacy and a 404 page.
- Light and dark themes with a saved preference, mobile navigation and Ctrl/⌘ K.
- Administration at `/admin`, including notice and document editors, content
  pages, attachments, a read-only audit log and outgoing mail history.
- Published content pages listed under `/stranky`, linked from the footer.
- Markdown page editor with preview, immutable revisions and edit conflict protection.
- Page image uploads with descriptions, draft previews and publication-aware access.
- Editable navigation and categories, subscriber evidence export, withdrawal and erasure.
- Email subscriptions default to all publications. Readers can select notice-board
  categories, uncategorized notices and general documents, then change their
  selection through the private link in each notification. Filtering applies to
  native publications, imports and messages waiting for delivery.
- Email password recovery and configurable password minimum, default 24 characters.

Production uses municipal content stored in the database for contact details,
municipal information, the calendar, accessibility and mandatory disclosures.
The full application uses structured event records for the calendar and homepage.
An optional [ČEZ outage importer](docs/cez-outages.md) keeps municipal outages,
affected addresses and source announcements in the same calendar.
The GitHub Pages demo keeps illustrative sample events.

## Backend and local tests

The backend includes authentication, attachment uploads, general documents,
content pages, scheduled publication and withdrawal, verified email subscriptions
and a durable mail queue. Audit entries are protected by database triggers.
The UI cannot edit or delete them. A controlled maintenance task removes expired
entries under the approved retention policy.

```bash
nix develop
./scripts/test.sh
```

Or use Docker Compose, including PostgreSQL and a test SMTP server:

```bash
docker compose --profile test run --build --rm tests
```

Documentation:

- [Local setup, administrator accounts, tests and API reference](docs/backend.md)
- [Database schema and migrations](docs/database.md)
- [Editorial workflows and password recovery](docs/editorial.md)
- [Outgoing mail setup and provider handover](docs/mail.md)
- [Deployment requirements and hosting considerations](docs/deployment.md)
- [Production operations, backups and recovery](docs/operations.md)
- [Ubuntu VPS staging deployment and SSH hardening plan](docs/vps-test-deployment.md)
- [Staging Compose modes, startup and scheduled import](docs/staging-deployment.md)
- [Remaining implementation work](docs/roadmap.md)
- [Original website migration and later host transfer](docs/migration.md)

The SSR website supports subscriptions, attachment downloads and published
content. The GitHub Pages preview uses **sample data** and sends no email.

## Run locally

The toolchain is pinned in `rust-toolchain.toml`. The current configuration uses
Rust 1.98.1 and cargo-leptos 0.3.7.

Initial setup:

```bash
rustup target add wasm32-unknown-unknown
cargo install cargo-leptos --locked --version 0.3.7
```

Start the development server with automatic rebuilding:

```bash
./scripts/dev.sh
```

The default listen address is **http://127.0.0.1:3000**. The script also supports
an existing cargo-leptos binary under `target/tools/`. That binary is not part
of the repository. The server connects to PostgreSQL and applies migrations on startup.
Sample data is inserted only when `OBEC_UKAZKOVA_DATA=true` is set explicitly.
`nix develop` starts PostgreSQL with persistent data in ignored `data/postgres/`.
Alternatively, run `docker compose up -d postgres mailpit`. Copy
`.env.example` to `.env` to configure local SMTP and privacy settings.

Open the site using the same origin as `OBEC_VEREJNA_URL`, which defaults to
`http://localhost:3000`. If you prefer `http://127.0.0.1:3000`, set that exact
value in `.env` as well. Origin checks use this setting.

Build:

```bash
cargo leptos build
cargo leptos build --release
```

A release build produces `target/release/obecni-web` and static assets under
`target/site`. Run the server from the project root, where `Cargo.toml` is
available. Running `cargo run` alone does not build the WebAssembly client or CSS.

## Standalone frontend preview and GitHub Pages

The `demo` feature runs Leptos entirely in the browser. It uses the same
components and design as the server version, with data from the versioned
`demo/documents.json` file. It does not include Axum or a database and requires
no API. Dates, archive states and example events are fixed at 29 September 2026.

Install the build tool and build for this GitHub repository:

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked --version 0.21.14
./scripts/build-demo.sh /vysker-modern/
```

The resulting `dist/` directory contains HTML, CSS, JavaScript, WebAssembly,
local fonts and images. It is ready for static hosting. Entry HTML files are
also generated for each page and sample notice detail, allowing direct links
and page refreshes without server URL rewriting. Unknown routes use `404.html`.

For hosting at a domain root, run `./scripts/build-demo.sh /`.
For local development of the standalone frontend:

```bash
trunk serve
```

The preview runs at http://127.0.0.1:8080. If your environment sets `NO_COLOR=1`,
use `env -u NO_COLOR trunk serve`. Trunk 0.21.14 expects a true/false value.

### Deploy the preview to GitHub Pages

1. Open **Settings → Pages → Build and deployment** in the repository.
2. Select **GitHub Actions** as the **Source**.
3. Push to `main` to run the `Frontend preview on GitHub Pages` workflow.
   You can also run it manually from Actions after changing the Pages settings.

The workflow reads the base path from Pages configuration and reports the
public URL after deployment. A personal access token is not required.

The demo sends no email and contains no attachment file contents. Search,
sorting, categories, the archive, theme switching and mobile navigation run in
the browser. The `ssr` production mode and `demo` preview are separate builds.

Basic environment settings:

| Variable | Default |
| --- | --- |
| `OBEC_ADRESA` | `127.0.0.1:3000` |
| `OBEC_MIN_PASSWORD_LENGTH` | `24` characters |
| `OBEC_DATABAZE_FILE` | Optional secret file, mutually exclusive with `OBEC_DATABAZE` |
| `OBEC_DATABAZE` | `postgresql://vysker:vysker@127.0.0.1:5432/vysker` |

## Product requirements

- Official notice board with an explicitly entered number of complete calendar
  days after the posting day, without a preselected interval.
- The shared public archive contains ordinary archived documents and documented
  removed postings whose documents remain public, excluding current or scheduled postings.
- Attachment availability after removal is an explicit mandatory choice with no default. Private attachments remain downloadable by administrators until separate retention disposal.
- A single document catalogue and administration, with separate posting history.
- Notifications about new documents, including official notices, activated
  after email verification.
- A read-only audit log with no editing or deletion controls in administration.
- Administration and creation of additional pages.
- A modern, clear interface with light and dark themes and accessible controls.
- Local Czech usage: **na Vyskři**.

These core workflows are implemented. Launch still requires real municipal
content, approved operating policies, hosting configuration and acceptance checks.
See the [remaining work](docs/roadmap.md).

## Canonical document registry

`documents` is the only document-content table. Both ordinary documents and
board documents have the same ID, detail and attachment owner. Migration 0029
physically merges the former `notices` content table, preserves numeric-ID
collisions through `document_legacy_ids`, and replaces `notices` with a narrow
compatibility projection of the same canonical rows.

`document_postings` records each local or source posting separately. Actual
posting, planned removal, actual removal, public visibility and permanent content
locking are independent. The first actual posting freezes content and ordered
attachment bytes through database guards. Publication requires a server-generated
preview fingerprint. Corrections and observed source revisions create new linked
concepts, never rewrite locked originals.

Access to attachment URLs is checked against the current canonical record at
request time. Removal can leave files public or restrict them to administration,
according to the mandatory choice. Full withdrawal also hides the detail and
search results. Historical migration/import does not send announcements, and
`document_announcements` prevents existing content being announced again.

See [migration and import](docs/migration.md), [editorial workflow](docs/editorial.md),
[board rules](docs/notice-board.md) and [measured verification](docs/unified-documents-verification.md).
Production rollout is a separate maintenance-window operation with a verified
backup and paused application/import/mail writers.

## Repository layout

```text
src/
  lib.rs               client entry point and shared modules
  main.rs              Axum server startup and shutdown
  app/                 Leptos pages, navigation and components
  app/admin/           login, lists, editors and authenticated API client
  content.rs           notice board data, server function and search
  catalog.rs           public documents, pages and subscriptions
  backend/             Axum API, authentication, domain logic, SMTP and workers
  bin/                 standalone API and administrator creation CLI
  preview.rs           standalone client entry point for Trunk
  config.rs            environment configuration
  db.rs                connections, migrations and sample data
  model/notices.rs     notice types, queries and date calculations
  notice_policy.rs     publication rules and minimum dates
migrations/            database migrations
tests/                 API, migration, SMTP and backup tests
docs/                  development and operational documentation
flake.nix, flake.lock   pinned Nix environment
compose.yaml           API, Mailpit and isolated test service
compose.production.yaml full website, HTTPS proxy and local backups
Dockerfile.web         production Leptos server and frontend build
deploy/                production configuration templates
seed-demo.sql          development sample content
style/main.css         responsive styles and both themes
public/                local fonts, illustrations, icons and theme initialization
scripts/dev.sh         development server
scripts/build-demo.sh  standalone build for static hosting
scripts/demo-routes.py entry HTML for direct links
scripts/database.py   consistent backups and recovery
demo/                  client HTML template and sample data
.github/workflows/     tests, production image checks and Pages deployment
design/                approved visual concepts and asset provenance
```

## Assets

Geist and Geist Mono are hosted locally. Their license is in
`public/fonts/LICENSE.txt`. Both relief illustrations were created with imagegen
from the approved concept. They are artistic illustrations, not surveyed terrain
models. The website loads no fonts or graphics from external CDNs.

The preview uses `noindex, nofollow`. Production pages containing notice board
data also restrict indexing through response headers. Real content, operating
configuration and the final accessibility statement must be approved before launch.

## Subscriptions and privacy

[Privacy documentation](docs/privacy.md) covers opt-in and opt-out, evidence of
subscription requests and controlled data retention.

## Production preparation

- [Notice board rules, archive and publication evidence](docs/notice-board.md)
- [Full Docker deployment, HTTPS, backups and recovery](docs/operations.md)
- Administration at `/admin/posta` shows recipients and outgoing message status.
- Mandatory disclosures, contact details and statements are managed as content pages.

See [Testing](docs/testing.md) for the API, browser and operational regression suites.
