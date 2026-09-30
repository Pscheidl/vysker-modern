# Outgoing mail configuration

Use a dedicated municipality-controlled SMTP account or transactional email
service. A personal administrator mailbox is not required. For example,
`web@vysker.cz` could be the sending identity if the municipality creates and
authorizes it. This is an example, not an existing configured account.

The selected service must allow the configured From address and provide SMTP
credentials. A website-specific password or service credential can be rotated
without changing a person's mailbox password.

## Configuration

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
3. Store the credential in the secret file and configure the canonical public URL.
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
