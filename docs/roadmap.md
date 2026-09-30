# Remaining implementation work

Repository review updated 30 September 2026. These are proposed next steps, not
claims that every item is a legal requirement or part of the original scope.
The core notice board, document management, verified subscriptions, read-only
audit and content page workflows are implemented.

Priority update, 30 September 2026: migration of the existing website is the last
planned implementation phase, after the agreed application and operational work.

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

The authorized application workflows listed above are implemented. Existing
website migration remains the final phase. SMTP account, sender authentication
and provider-specific delivery handling remain part of the deployment handover,
as described in [mail.md](mail.md).

## Configuration and approvals outside application code

The municipality and operator still need to provide actual content, approve
publication and retention rules, establish the link to official records management,
select hosting, configure the domain and SMTP, assign operational ownership and
complete accessibility and security acceptance. These are launch tasks even when
no additional application feature is needed.

## Final phase: migrate the existing website

Begin migration after the agreed features, production configuration, monitoring
and backup/recovery work are ready. Pavel already has an existing website crawler
in [Pscheidl/scrape](https://github.com/Pscheidl/scrape), available locally at
`/home/pavel/dev/scrape`, with SSH remote `git@github.com:Pscheidl/scrape.git`.
Use this implementation as the starting point.

Source review on 30 September 2026 identified a Rust crawler using SQLx and
PostgreSQL. It discovers pages through the website menu, extracts attachment links
from the Vismo content area and stores page titles, URLs, attachment link names
and discovery timestamps in the `pages` and `assets` tables. It also sends email
notifications about newly discovered attachment links.

The current code does not persist page bodies or download attachment files.
Migration work will need to extend extraction and storage accordingly, check
coverage of nested detail pages and archives, and capture original publication
and withdrawal dates where available. The `first_visited` and `observation_time`
fields record crawler discovery times, not original publication dates. These
findings come from reading the source, without running the crawler.

Adapt its output into an import of active notices, documents and pages. Preserve
source URLs, attachments and available publication evidence, prevent duplicate
imports and prepare redirects for existing public URLs. Verify the imported
content and notice board workflow before switching the domain.

See [deployment.md](deployment.md), [operations.md](operations.md) and
[hosting.md](hosting.md).
