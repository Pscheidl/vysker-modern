# Newsletter subscriptions and privacy

## Subscribe and unsubscribe

A visitor enters an email address and selects **Přihlásit k odběru** (Subscribe).
The form identifies the purpose and controller, explains withdrawal and links
to full privacy information. Submitting the request is an active opt-in. No
additional checkbox is required. The [ÚOOÚ newsletter form](https://uoou.gov.cz/cs/newsletter-wv/registration-confirmation)
uses the same approach.

Verification email helps prevent someone from subscribing another person's
address. A subscription becomes active only after confirmation. Opening the
link alone changes nothing, so an antivirus link scanner cannot activate or
cancel it. Confirmation and withdrawal each use one button and require no
account or password. Unsubscribe links in new messages have no fixed expiry.
Verification links expire after 24 hours, and a new request invalidates the
previous verification link.

The backend stores the exact notice wording and privacy configuration, their
fingerprint, and request, confirmation and withdrawal times. This is evidence
of opt-in and opt-out, with no additional step for the visitor. The historical
information shown during confirmation matches the original request. Editing
today's policy cannot expand the purpose of an earlier subscription.

`GET /api/v1/privacy` returns the current information and `fingerprint`.
Subscribe using:

```json
{"email":"resident@example.test","consent":{"fingerprint":"fingerprint-from-GET-privacy"}}
```

Read-only evidence is available through the authenticated
`GET /api/v1/admin/subscribers/{id}/consents` endpoint. Legacy addresses without
opt-in evidence do not receive notifications and must subscribe again. Migration
does not invent prior consent or send automatic requests for it.

## Configuration

`OBEC_PRIVACY_CONFIG` points to JSON matching `config/privacy.example.json`.
The example contains explicit placeholders and development retention periods.
The operator must supply controller details, contacts, processors, international
transfers, legal bases and justified retention periods before setting `approved_on`.
Controller name and a valid contact email are required. A postal contact address
can be supplied as `controller_address`, or omitted or left empty. If the operator
has no appointed data protection officer, omit `dpo_email` or leave it empty. Otherwise
provide the officer's valid contact email. The public notice and confirmation page
show that contact only when supplied.

Without a policy, subscriptions are disabled. `OBEC_PRODUCTION=true` requires
an approved configuration without placeholder values, HTTPS, SMTP TLS and disabled
sample data. These checks do not replace approval of the actual policy contents.

Maintenance on a 30-second schedule removes expired pending requests, withdrawn
subscribers, associated evidence, tokens and mail records. Active subscriptions
continue until withdrawal. A short lease protects messages currently being sent.
Withdrawal cancels pending mail. A message already accepted by SMTP cannot be
recalled. `sent_at` records SMTP acceptance, not reading or final inbox delivery.

Audit history has no edit or delete API. Database triggers prevent modification.
The exception is controlled, transactional deletion of entries older than the
approved retention period. Cleanup summaries contain no erased addresses.

`notice_internal_days` controls removal of internal notice text and evidence
after a recorded withdrawal. Historical archive entries without a withdrawal
timestamp are not automatically purged by this rule. The minimal public record
always remains. The production
backup service uses a positive `backup_days` value to rotate local snapshots.
Use `backup_days: 0` to declare that the operator does not create backups of the
subscriber database, and keep the
backup service disabled in that deployment. This value changes the published
privacy information, it does not stop an already running backup service or delete
existing backups. If backups are enabled, configure a positive retention period
and encrypted offsite backup retention. The operator must also configure time-based
diagnostic log rotation (`operational_log_days`).

After restoring an older backup, mail must remain stopped until subsequent
withdrawals and erasure requests have been accounted for. The provided recovery
tool discards subscriptions from the restored copy. A backup does not extend
the right to contact subscribers. See [operations.md](operations.md).

## Security and visitor information

Application HTTP logs omit query strings, headers, request bodies and visitor
addresses. Verification tokens are therefore not copied into those access logs.
The reverse proxy and external monitoring must follow the same policy.

Responses include `Referrer-Policy: no-referrer`, `nosniff`, frame protection
and HSTS in production. Administration uses a technical session cookie. Theme
preference is stored locally in the browser. Analytics and advertising cookies
are not part of the application. Additional integrations require reassessment.

References: [ÚOOÚ data protection handbook](https://uoou.gov.cz/verejnost/zakladni-prirucka-k-ochrane-udaju),
[EDPB on lawful processing and consent](https://www.edpb.europa.eu/sme/be-compliant/process-personal-data-lawfully_en),
[ÚOOÚ on cookies](https://uoou.gov.cz/verejnost/qa-otazky-a-odpovedi/cookies).
