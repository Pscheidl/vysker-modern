# Municipal website deployment

Working plan dated 29 September 2026. Choosing a hosting provider does not by
itself establish legal compliance. This document describes the operating model
and launch preparation. It is not a completed audit or legal certification.

## Hosting

For Vyskeř, obtain a quote for a managed Linux server, backups and monitoring
from a provider experienced with public administration. One previously identified
candidate is [CRA / Cloud4com](https://www.cra.cz/ict-reseni/cloud), offering Czech
data centres and infrastructure services. Confirm current service scope, pricing
and SLA in a specific quote. Managing a virtual server may not include maintaining
this Rust application. See [hosting research](hosting.md) for the Webhouse/VisMo
findings and questions to resolve before procurement.

The municipality must first classify its website and electronic notice board
within its information systems. DIA describes a website as potentially a separate
public administration information system (ISVS), or functionality of another ISVS.
See [DIA's ISVS classification guidance](https://archi.gov.cz/znalostni_baze%3Aco_je_neni_isvs).

If cloud operation falls under Act No. 365/2000 Coll., the service selection must
follow the applicable eGovernment cloud rules. Verify the exact service, provider
and required security level in the DIA catalogue. A Czech server location or a
VPS product name is not sufficient. Supply chain and municipal registration
obligations also need review. See [DIA guidance](https://www.dia.gov.cz/cs/nase-cinnosti/na-cem-pracujeme/egovernment-cloud/metodiky-navody-formulare/otazky-a-odpovedi-1)
and the [cloud catalogue](https://www.dia.gov.cz/cs/nase-cinnosti/na-cem-pracujeme/egovernment-cloud/katalog-cloud-computingu/katalog-cloud-computingu).

The contract should define data and domain ownership, application maintenance,
incident response, recovery, service exit and data export. Where the supplier
processes personal data, include the requirements of GDPR Article 28 and terms
for subprocessors. See [ÚOOÚ guidance on processors](https://uoou.gov.cz/poradna/poradna-gdpr/zpracovatel).

## Proposed technical setup

- Municipality-controlled domain and HTTPS through Caddy.
- One Leptos SSR and Axum instance deployed using Docker Compose.
- SQLx and PostgreSQL 18, with a persistent database volume or managed instance.
- Consistent PostgreSQL backups and encrypted offsite copies with an agreed retention period
  and regular recovery exercises.
- Monitoring of availability, certificates, free space, errors, publication and
  withdrawal schedules, and the mail queue.
- Contracted SMTP with TLS, SPF, DKIM and DMARC.
- Individual administrator accounts, a second factor or controlled VPN access,
  regular updates and an assigned operator.
- A separate preview environment with sample data for development and review.

These are proposed operating measures, not a statement that each item is an
explicit legal duty for every municipality. Availability, acceptable data loss
and recovery time determine whether a standby instance or replication is needed.

`Dockerfile.web` builds the complete website. Production Compose includes HTTPS
and automatic local backups. The [operations manual](operations.md) covers setup,
recovery and remaining operator actions. The original `Dockerfile` and Compose
serve the standalone backend and tests. GitHub Pages remains a static preview.

## Before launch

### Accessibility and mandatory disclosures

Assess the website and published documents against Act No. 99/2019 Coll. and
the applicable EN 301 549 standard. Cover keyboard navigation, screen readers,
forms, zoom, contrast, both themes and PDFs. Publish an accurate accessibility
statement with a feedback contact. See [DIA legislation and standards](https://www.dia.gov.cz/cs/nase-cinnosti/na-cem-pracujeme/pristupnost-internetovych-stranek-a-mobilnich-aplikaci/legislativa).

Provide current mandatory information under Act No. 106/1999 Coll. and the
structure in Decree No. 515/2020 Coll., including contacts, the electronic filing
office, data mailbox and relevant annual reports. The [Ministry of the Interior's
information page](https://mv.gov.cz/povinne-zverejnovane-informace) illustrates the
structure. The municipality must approve the real content.

### Notice board and archive

Publication rules, pre-publication review, public archive minimization and
publication evidence are implemented. See [notice-board.md](notice-board.md)
for behavior and municipal responsibilities.

The 15-day default is not a universal legal rule. Select the appropriate profile
for each document, including its date calculation and restrictions on early
withdrawal. Municipal property intentions and council meeting announcements have
different minimum intervals. See [Ministry guidance](https://mv.gov.cz/povinnosti-pri-nakladani-s-majetkem-obce).
Agree on outage handling and evidence of actual publication with the municipality.

After withdrawal, a minimal record remains. Its public form must respect the
purpose and permitted duration of personal data publication. Titles, descriptions
and attachment names can contain personal data. Selecting archive retention does
not justify publishing those data indefinitely. See [ÚOOÚ on notice boards](https://uoou.gov.cz/poznatek-z-dozorove-cinnosti-k-dorucovani-pisemnosti-na-uredni-desce).

The public archive does not replace official records management. Removing a web
attachment must be separated from retaining the official original and evidence.
Agree on this workflow with the responsible records officer. See [Ministry
records management materials](https://mv.gov.cz/archivnictvi-a-spisova-sluzba-2).

### Privacy, subscriptions and security

With the data protection officer, define processing purposes, legal bases and
retention periods for subscribers, logs and audit records. Complete visitor
information. Voluntary subscriptions use a button opt-in, email verification and
account-free withdrawal. There is no additional checkbox. The application records
notice wording and subscription transitions. See [privacy.md](privacy.md) and
[ÚOOÚ's handbook](https://uoou.gov.cz/verejnost/zakladni-prirucka-k-ochrane-udaju).

Using only strictly necessary technical cookies does not require a consent banner,
but visitors still need information. Reassess when adding analytics, external
maps or video. See [ÚOOÚ on cookies](https://uoou.gov.cz/verejnost/qa-otazky-a-odpovedi/cookies).

Determine cybersecurity obligations from the applicable regulated services.
Do not automatically apply the duties of municipalities with extended powers
to every municipality. Consult [NÚKIB's information and scope guidance](https://nukib.gov.cz/cs/infoservis/aktuality/2372-ohlaseni-podle-noveho-zakona-o-kyberneticke-bezpecnosti-provedlo-pres-4800-organizaci/).

## Launch sequence

1. The municipality, operator and data protection officer confirm system
   classification, content duties, publication rules and retention.
2. Obtain a specific hosting quote covering management, backups, SMTP and data
   protection. Verify catalogue entries where applicable and contractual responsibilities.
3. Complete agreed application work and configure production hosting.
4. Check accessibility, security, monitoring and recovery. Obtain municipal
   approval of the prepared content.
5. As the final implementation phase, migrate the existing website using
   [Pavel's existing crawler](https://github.com/Pscheidl/scrape) as the starting
   point. Review its output and adapt the import, preserve available publication
   evidence and prepare redirects for old URLs. Verify the imported content,
   attachments and active notices, including accessibility and the notice board
   workflow.
6. Switch the domain after migration and final acceptance.
