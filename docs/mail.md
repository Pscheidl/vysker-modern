# Outgoing mail configuration

Use a dedicated municipality-controlled SMTP account or transactional email
service. A personal administrator mailbox is not required. For example,
`web@vysker.cz` could be the sending identity if the municipality creates and
authorizes it. This is an example, not an existing configured account.

The selected service must allow the configured From address and provide SMTP
credentials. A website-specific password or service credential can be rotated
without changing a person's mailbox password.

## Google from the administration

Production can start without any SMTP credentials in deployment files. Follow
the [deployment preparation](operations.md#deployment) with the
`compose.google-mail.yaml` overlay and Docker Compose 2.24.4 or later:

```bash
docker compose --env-file deploy/.env -f compose.production.yaml -f compose.google-mail.yaml up -d
```

Use both Compose files for administrator creation, preparation and later updates.
Leave `SMTP_*` and `EMAIL_FROM` unset and do not create `smtp-password.txt`.
The overlay removes that secret mount and SMTP login environment entries.
Messages remain queued until you save working Google credentials below. Returning
to server settings while using this overlay returns to an unconfigured sender.
The original production file alone keeps the server-managed SMTP setup.

Open **Rozesílání → Nastavení odesílání** (`/admin/posta/nastaveni`) and select
**Google**. Enter the full Gmail or Google Workspace mailbox address, the sender
name and a Google app password. Confirm the change with your website administrator
password. The application selects `smtp.gmail.com`, port 587 and required STARTTLS
automatically. The From address is the configured Google mailbox.

Create the app password at <https://myaccount.google.com/apppasswords> after
enabling two-step verification. Paste the 16-letter password with or without its
display spaces. Use the app password, not the normal Google account password.
Google Workspace policy and some account protection settings can make app
passwords unavailable. A Google API key alone cannot authorize sending. Accounts
that require OAuth need a separate OAuth integration, which this form does not
provide. See [Google's app password help](https://support.google.com/accounts/answer/185833)
and [Gmail authorization](https://developers.google.com/workspace/gmail/api/auth/web-server).

Save first, then send a test message. It goes only to the signed-in administrator's
email address. A successful test means SMTP acceptance, so also check that mailbox.
Saving does not send a test automatically. Settings apply to subscription and
password recovery messages without restarting the server. A message already being
submitted may finish with the previous settings.

The password field stays empty on subsequent visits. Leave it empty to keep the
saved credential for the same Google mailbox. Switching the mailbox requires a new
app password. Selecting server settings removes the stored Google configuration
and returns to the environment configuration below.

Credentials are encrypted in PostgreSQL with an authenticated encryption key
stored separately in `OBEC_MAIL_SETTINGS_KEY_FILE` (default
`data/mail-settings.key`). The server creates that key on first save with owner-only
permissions. Production and development Compose keep it in the persistent
`mail-secrets` volume. Mount the same key for every application replica.
Include the key in protected operational backups separately from database dumps.
Losing it prevents use of saved Google credentials, so restore the original key
or reset to server settings and enter a new app password. The application never
returns the stored password through the API or writes it into audit records.

## Server configuration

| Setting | Meaning |
| --- | --- |
| `OBEC_SMTP_HOST` | Provider SMTP hostname |
| `OBEC_SMTP_PORT` | Provider port, usually 587 for STARTTLS or 465 for implicit TLS |
| `OBEC_SMTP_TLS` | `starttls` or `tls` in production, `none` only for local Mailpit |
| `OBEC_SMTP_UZIVATEL` | Account or service username |
| `OBEC_SMTP_HESLO_FILE` | Secret file containing the SMTP credential |
| `OBEC_EMAIL_OD` | Authorized sender, for example `Vyskeř <web@vysker.cz>` |
| `OBEC_VEREJNA_URL` | Canonical HTTPS website URL used in email links |

Do not commit credentials. Production Compose reads
`deploy/secrets/smtp-password.txt` through a Docker secret and passes the other
settings from `deploy/.env`. Its default transport is STARTTLS on port 587.
If a provider requires implicit TLS, change the Compose transport to `tls` and
its port to 465 together. Complete the approved privacy configuration before
turning on public subscriptions.

## Before public launch

1. Select the provider and create the sender/service account.
2. Configure the provider-specified SPF and DKIM records, then an appropriate
   DMARC policy for the sending domain. Preserve existing authorized senders.
   Exact DNS values must come from the chosen provider and domain administrator.
3. Save Google credentials in administration or store the server-managed SMTP
   credential in its secret file, and configure the canonical public URL.
4. Check verification, a document notification, unsubscribe and administrator
   recovery with controlled recipient addresses. Confirm sender authentication
   in the received message headers and check spam handling.
5. Agree sending limits, handling of permanent failures, bounces and complaints,
   operational ownership and the process for credential rotation.

Local tests use Mailpit and disposable data. They prove SMTP submission and
application behavior, not deliverability from the future production provider.

## History and operations

**Rozesílání** lists subscription and recovery messages, recipient, queue status,
attempt count and SMTP acceptance time. Acceptance does not prove inbox delivery
or reading. Security messages use `recovery_mail`, subscription messages use
`mail_queue`. Bodies containing links are cleared after sending or cancellation.
The monitor includes both queues in backlog and stalled-delivery checks.

Retries have stable Message-IDs and at-least-once semantics. A process crash after
SMTP acceptance but before recording it may cause a duplicate. Provider bounce
or complaint webhooks are not yet integrated and must be designed against the
selected provider. Subscription retention follows the approved privacy policy.
Recovery message metadata is kept for 30 days.

Recovery backlog alerts default to 10 minutes, before the 30-minute link expiry.
Unsent links that expire retain a separate failure timestamp and raise an alert
for the following 24 hours. Deliberate cancellation after a password change is
not classified as a delivery failure.
