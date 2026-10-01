# Running the full website

## Deployment

`Dockerfile.web` builds the Leptos SSR server, WebAssembly client, CSS, images,
fonts and administrator CLI. `compose.production.yaml` adds Caddy with HTTPS,
a persistent database, daily local backups and an administration network allowlist
for the office or VPN. `compose.yaml` remains the development and test setup.

For Google configured entirely through administration, use the optional
`compose.google-mail.yaml` overlay with Docker Compose 2.24.4 or later.
Add `-f compose.google-mail.yaml` immediately after `-f compose.production.yaml`
in every command below, including administrator creation and local preparation.
Leave `SMTP_*` and `EMAIL_FROM` unset and skip `smtp-password.txt`.
The overlay removes the SMTP credential environment entries and secret mount,
using Docker's [reset and override rules](https://docs.docker.com/reference/compose-file/merge/).
Database and privacy secrets are still required. During local preparation,
configure Google in **Rozesílání → Nastavení odesílání** and send a test.
Until then, messages remain queued. With this overlay, returning to server
settings also leaves messages queued until Google is configured again.

1. Copy `deploy/.env.example` to the ignored `deploy/.env` file. Set the domain,
   specific `ADMIN_NETWORKS` and, for server-managed SMTP, its settings and sender. The example `.test`
   domains and `192.0.2.1` address are placeholders.
2. Add municipality-approved `privacy.json` under the ignored `deploy/secrets/`
   directory. Server-managed SMTP also requires `smtp-password.txt`. The password file must be readable
   by container UID 10001. Restrict host directory access to the operator.
   Add separate random passwords in `postgres-password.txt` (database operator)
   and `database-password.txt` (application role). Put
   `postgresql://vysker:URL_ENCODED_APP_PASSWORD@postgres:5432/vysker`
   in `database-url.txt`, using the same application password. Make this URL file
   readable by UID 10001. The application role has no superuser or database-creation
   privileges. PostgreSQL initialization reads the password files only when the
   database volume is empty. Later password rotation requires `ALTER ROLE` and a
   matching updated URL secret.
   Compose secrets are mounted files, not automatic disk encryption.
3. Check that `172.30.64.0/29` does not overlap existing infrastructure. If you
   change it, update the proxy's fixed address and `OBEC_TRUSTED_PROXY` as well.
4. Build the image and create an administrator. Enter the password in the hidden prompt.

```bash
docker compose --env-file deploy/.env -f compose.production.yaml build web
docker compose --env-file deploy/.env -f compose.production.yaml run --rm web ./obec-admin admin@your-domain.cz
```

Before the first production start, populate real content in local mode using the
same PostgreSQL database:

```bash
docker compose --env-file deploy/.env -f compose.production.yaml run --rm \
  -p 127.0.0.1:3000:3000 \
  -e OBEC_PRODUCTION=false \
  -e OBEC_VEREJNA_URL=http://localhost:3000 web
```

Use an SSH tunnel to the server's local port. Keep this preparation server off
the public internet. Sign in at `/admin` and publish pages with these slugs:

- `kontakt`: contacts, electronic filing office, data mailbox and office hours
- `obec`: real information about the municipality and its office
- `kalendar`: actual events or an accurate statement that none are listed
- `pristupnost`: an approved accessibility statement based on an assessment
- `povinne-informace`: mandatory disclosures using Decree No. 515/2020 Coll.

Production startup requires these pages. Administration cannot hide or rename
them in production, but their content can be edited. The application cannot
verify the accuracy or legal completeness of the text. Additional published
pages appear under **Informace obce** (Municipal information) in the footer.
Production does not fall back to sample contacts, events or accessibility text.

Stop the preparation server, then start production:

```bash
docker compose --env-file deploy/.env -f compose.production.yaml up -d
```

For Google configured through administration, the production start command is:

```bash
docker compose --env-file deploy/.env -f compose.production.yaml -f compose.google-mail.yaml up -d
```

Caddy requires correct DNS and reachable ports 80 and 443. The application port
is not published. `ADMIN_NETWORKS` must contain only approved ranges. Do not
use `0.0.0.0/0`. An additional proxy or CDN requires a review of trusted IPs,
access rules and logs. The application accepts `X-Real-IP` only from the single
configured proxy, which always overwrites that header.

CI checks the production image by starting it and downloading JS, WASM and CSS.
Verify the real TLS certificate and SMTP service on the deployment host. Set up
SPF, DKIM and DMARC for the sender.

## Synchronization during the website transition

The image also contains `scripts/legacy_sync.py`. While the old website is still
the content source, a host cron entry can call it through `docker compose exec`.
It adds new items without duplicates, queues changed items for review and keeps
disappeared items. See [the synchronization procedure](migration.md#periodic-synchronization-while-both-websites-run)
and `deploy/legacy-sync.cron.example` for the first run, scheduling and review.
The private `migration-state` volume holds reports. Pending proposals and their
captured bytes live in PostgreSQL and are included in database backups.

## Backups and recovery

The `backup` service creates a consistent PostgreSQL custom archive daily using
`pg_dump`. It checks that `pg_restore` can read the archive directory before
publishing the file atomically. This does not replace a full restore rehearsal.
Retention comes from the approved `retention.backup_days` value. Files have mode
0600. Supplement local backups with encrypted offsite copies and monitoring.

Google sending configured in the administration also needs the separate
`mail-secrets` volume, containing `/app/data/mail-settings.key`. Database dumps
contain the encrypted credential but do not contain this key. Back up the key
separately with restricted access and restore it when moving the application.
All web replicas must share the same key. See [mail.md](mail.md) for configuration
and recovery when the key is lost.

Create a manual snapshot:

```bash
python3 scripts/database.py backup --directory data/backups --keep-days 30
```

The value 30 is an example. Use the approved period. The tool never overwrites
an existing destination. Rotation applies only to `vysker-*.dump` files in the
chosen directory, which must be dedicated to these backups.

Restore **into a new PostgreSQL database** after stopping both the application and backup
service. Stop external mail delivery and integrations before switching databases.

```bash
python3 scripts/database.py restore \
  --source data/backups/vysker-SELECTED-SNAPSHOT.dump \
  --destination postgresql://OPERATOR@localhost:5432/vysker_restored \
  --discard-subscriptions
```

Backup uses `OBEC_DATABAZE` or `OBEC_DATABAZE_FILE`. Recovery requires an operator
account with permission to create a database. Supply its password through
`.pgpass` or a secret URL file, rather than command arguments. The example URL
omits the password. Recovery refuses any existing destination and removes only
its own new database if restoring or sanitizing it fails.

The required flag removes subscribers, consent evidence, tokens and the mail queue
from the restored copy. Subscribers must opt in again. This prevents reactivating
people who unsubscribed after the snapshot. Administrator sessions are invalidated
too. Outstanding password recovery links and security mail are removed when
present, including when restoring a backup from before those tables were added.
The original database and snapshot remain unchanged.

Review the restored database, especially publication states and dates, before
switching `OBEC_DATABAZE` and reopening the service. Rehearse recovery regularly.

## Monitoring

- `/api/v1/health` checks the database and HTTP service.
- `/api/v1/ready` also returns 503 if maintenance has not succeeded within the
  last 120 seconds. Compose uses this endpoint.
- External monitoring should check the real domain, HTTPS and download of a known
  public document. Local events do not measure internet availability.
- Administration under **Rozesílání** (Outgoing mail) shows recipients, subjects,
  attempt counts and message states during retention. It exposes no bodies or
  tokens. SMTP acceptance does not prove final delivery or reading.
- Monitor queue growth, repeated SMTP failures, free disk space, backup age,
  certificate expiry and drafts produced by missed publication schedules.

Proxy access logging is disabled, and error log filtering removes request data.
The application does not log token URLs or recipient addresses. Compose limits
log size. The operator must configure time-based deletion using the approved
`operational_log_days` setting in the host or logging system, and review logs
kept by SMTP, firewall and hosting providers.

## Municipality and operator responsibilities

Before launch, approve content and publication rules, coordinate the physical
notice board and official records management system, establish retention and
disposal rules, assess website and document accessibility, classify the system
and applicable cybersecurity duties, sign the hosting agreement and assign an
operator. Configure VPN access, the domain, SMTP, external monitoring and encrypted
offsite backups. The application cannot approve these decisions for the municipality.

Internal notice data and publication evidence use the separate
`retention.notice_internal_days` period, measured from actual withdrawal.
After expiry, maintenance removes internal text, attachment names and old evidence,
while keeping the minimal notice record. The municipality must arrange transfer
to official records management and choose a suitable retention period. The example
365 days is not a legal recommendation.

## Operational alerts

The production Compose stack includes a `monitor` service. Every minute it checks
application readiness, database readability, the local backup directory, free
space on both data and backup volumes, and the pending SMTP queue. It reads the
database in a read-only transaction and emits no email addresses, document text or tokens.
Default alert thresholds are:

- No completed backup, or the latest snapshot is older than 30 hours
- Less than 1,024 MiB or 10 percent free on either storage volume
- At least 1,000 pending messages, or an unsent message older than 30 minutes
- An unreachable or unready application, or an unreadable database

Change thresholds using the command arguments listed by `python3 scripts/monitor.py
--help`. Backups are created and archive-directory-checked by `database.py`. The monitor
checks their presence and age, not their content or recoverability. A successful
restore rehearsal remains necessary.

Without a webhook, alerts go to container logs. To send alerts to an operator's
HTTPS webhook, put its URL in the ignored `deploy/secrets/alert-webhook.txt`, then
include the optional overlay:

```sh
docker compose --env-file deploy/.env -f compose.production.yaml \
  -f compose.monitor-alerts.yaml up -d --build
```

The webhook receives JSON with `checked_at`, `status`, `alerts` and aggregate
`metrics`. Ensure the selected receiver accepts this schema or configure its
adapter. Notifications are sent when failures change, hourly while a failure
persists, and once on recovery. Failed deliveries are retried on the next check.
The monitor persists deduplication state in its own volume. Secret URLs and
response bodies are omitted from logs. Redirects are refused. The container
healthcheck fails for an outstanding alert, a failed notification, or a stale
monitor heartbeat.

For a one-off check on a host:

```sh
python3 scripts/monitor.py check \
  --backups data/backups --ready-url http://127.0.0.1:3000/api/v1/ready
```

The database connection uses `OBEC_DATABAZE` or `OBEC_DATABAZE_FILE`.
`--database-disk PATH` checks the actual local PostgreSQL volume if mounted.
For managed databases, monitor database disk space through the provider instead
of reporting the application container's disk as database storage.

A failed check exits with code 2. No webhook is called in `check` mode.
Schedule an **independent external monitor** for the public HTTPS endpoint
`/api/v1/ready`, for example every minute with an alert after two consecutive
failures. The local monitor cannot report a complete host or network outage.
The operator still has to supply and verify the real notification destination.

SMTP processing now runs independently of notice publication and retention
maintenance. A slow mail server does not hold back the maintenance heartbeat.

## Administrator password policy

Passwords use Argon2id with a new cryptographically random salt for every hash.
Only the PHC encoded hash, including salt and algorithm parameters, is stored.
`OBEC_MIN_PASSWORD_LENGTH` controls the minimum number of Unicode characters,
with a default of **24**. Values must be between 1 and 1024. The independent
maximum is 1024 UTF-8 bytes. Set the environment value and restart the application
and any administrator CLI process. Browser forms read the policy from the current
session response. Raising the minimum applies to new passwords, not verification
of existing passwords. Account creation, password changes, email recovery and operator recovery
all enforce the same setting.


SMTP account and domain setup are documented in [mail.md](mail.md). Administrator
email recovery is documented in [editorial.md](editorial.md). Both mail queues
are monitored, and database restore also discards outstanding recovery tokens
and security messages.
