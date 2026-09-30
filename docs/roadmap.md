# Remaining implementation work

Repository review updated 30 September 2026. These are proposed next steps, not
claims that every item is a legal requirement or part of the original scope.
The core notice board, document management, verified subscriptions, read-only
audit and content page workflows are implemented.

Priority update, 30 September 2026: the requested migration tooling is implemented
and the existing public website has been imported into an isolated local preview.
Content acceptance and the final domain switch remain deployment tasks.

## Prioritize before public launch

| Area | Current state | Next step |
| --- | --- | --- |
| Administrator account lifecycle | Account UI, password changes, operator recovery, deactivation and session revocation are implemented and audited | Agree role separation or MFA if the access model changes |
| Public search and pagination | Server pagination, Czech search and independent notice details are implemented | Measure larger data sets before introducing a full-text index |
| Operational alerts | Local checks for readiness, backup age, disk and mail queue, plus optional webhook delivery | Configure the real receiver and independent external monitoring with the operator |
| Offsite recovery | Consistent local snapshots and tested restore exist | Automate encrypted offsite copies and rehearse recovery on a separate host |
| Frontend regression coverage | Playwright workflows cover publishing, download, opt-in, opt-out, keyboard navigation, search and mobile layout | Perform accessibility assessment against real content and assistive technology |

Production currently restricts administration to allowed office or VPN networks.
If broader access is required, implement an appropriate second factor or managed
identity integration. Role separation and editorial approval are useful if several
people will publish content, but the required workflow must be agreed first.

## Implemented editorial workflows

- Password recovery with expiring single-use links, durable security email and
  session revocation. Password minimum is configurable, default 24 characters.
- Restricted Markdown page editing, preview, immutable revision history and
  optimistic conflict detection. Older content can be loaded for review and saved.
- Navigation and category management, including ordering, safe local destinations
  and protection of categories referenced by notices.
- Structured calendar events, Prague time, cancellation, public details, archive
  and live homepage feed.
- Subscriber search, filters, evidence export, withdrawal and erasure with
  reauthentication, audit and active SMTP lease protection.

See [editorial.md](editorial.md) for the complete workflows.

## Mail and file handling

- Decide how to process permanent delivery failures, bounces and complaints with
  the selected SMTP provider. Current history records SMTP acceptance and retry
  counts, not final delivery or each individual attempt's response.
- Mail processing and maintenance now have independent workers. Monitor delivery
  latency with the operational checks and agree escalation with the operator.
- Consider attachment scanning and a review queue according to the upload policy.
  Current validation checks file types and signatures, not malicious content.
- Add object storage only if attachment volume or the hosting model warrants it.
  The current transactional PostgreSQL BYTEA storage is part of the backup design.

## PostgreSQL baseline

PostgreSQL is now the database target, including SQLx queries, schema guards,
backup/restore tooling, Nix and Compose instances, and isolated integration tests.
The initial schema replaces the SQLite migrations because the application has
not been deployed. Database setup is documented in [database.md](database.md).

The authorized application workflows listed above and migration tooling are
implemented. SMTP account, sender authentication
and provider-specific delivery handling remain part of the deployment handover,
as described in [mail.md](mail.md).

## Configuration and approvals outside application code

The municipality and operator still need to provide actual content, approve
publication and retention rules, establish the link to official records management,
select hosting, configure the domain and SMTP, assign operational ownership and
complete accessibility and security acceptance. These are launch tasks even when
no additional application feature is needed.

## Existing website migration

The tools build on the menu and content discovery used by Pavel's
[scraper](https://github.com/Pscheidl/scrape), without running its notification
worker. They capture full page bodies and files, validate checksums, import into
PostgreSQL with immutable provenance, preserve available dates, rewrite links,
create structured calendar entries and resolve historical URLs through HTTP 301.
Repeated imports skip unchanged records and refuse source conflicts.

Before launch, reconcile the failed original link and empty source pages, review
fragment links and archived document classifications, approve content and verify
active notice publication continuity. The current capture found no active notices
to transfer. Take a fresh snapshot and reconcile later changes before cutover.
Private subscriber lists, old accounts and original audit records are not present
in the public source and cannot be recovered by crawling it.

See [migration.md](migration.md) for the operator workflow and acceptance steps.

See [deployment.md](deployment.md), [operations.md](operations.md) and
[hosting.md](hosting.md).
