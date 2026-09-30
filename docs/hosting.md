# Hosting research: Webhouse, VisMo and this application

Reviewed on 29 September 2026. This note distinguishes published evidence from
items that still require a current offer or written confirmation. No hosting
service has been selected or ordered.

## Webhouse and VisMo

VisMo is the CMS offered by the Czech company **WEBHOUSE, s.r.o.**, company ID
25327054. Its VisMo hosting and maintenance package is a managed CMS service.
The public product description does not establish that it can host an arbitrary
third-party Rust application or Docker Compose deployment. See the
[official VisMo 6 product page](https://webhouse.cz/redakcni-system-vismo-6).

### Published infrastructure evidence

The Webhouse service contract for Boskovice from 2024 includes a processor annex
identifying **OptoNet Communication** for data servers, **VSHosting** for email,
and **INTERNET CZ** for domain services. The annex itself is dated 31 March 2021.
It establishes that supplier arrangement for the published contract, not the
current location of every VisMo deployment. Source: [contract and annex, pages 5–6](https://smlouvy.gov.cz/smlouva/soubor/36027532/195-2024%20-%20WEBHOUSE%2C%20s.r.o.%2C%20Praha%20-%20Smlouva%20o%20provozu%20a%20servisu%20-%20provoz%20a%20servis%20webov%C3%BDch%20str%C3%A1nek%20m%C4%9Bsta%20Boskovice.pdf).

OptoNet advertises **Datové centrum Vysočina**. This supports identifying an
infrastructure provider to investigate. It does not establish the physical site
of a particular municipality's servers, replicas or backups. See
[OptoNet's website](https://optonet.cz/).

Ask Webhouse for current production and backup locations, the applicable list
of subprocessors and whether the arrangement differs between VisMo versions.

### What can be confirmed about compliance

- DIA lists WEBHOUSE, s.r.o. as registered cloud provider **146**, with registration
  dated **10 April 2025**. See the [official provider register](https://www.dia.gov.cz/cs/nase-cinnosti/na-cem-pracujeme/egovernment-cloud/katalog-cloud-computingu/poskytovatele-cloud-computingu).
- DIA distinguishes provider registration from registration of a specific cloud
  offer. A provider record alone is not evidence that every product is registered
  for the security level required by a particular information system.
- A specific VisMo offer and security level were **not verified in this review**.
  No Webhouse match was found on the retrieved current [offers listing](https://www.dia.gov.cz/cs/nase-cinnosti/na-cem-pracujeme/egovernment-cloud/katalog-cloud-computingu/nabidky-cloud-computingu).
  This finding is not proof of absence from every catalogue view or a conclusion
  that an existing deployment is unlawful. Request the exact offer identifier.
- Webhouse states that it holds ISO 27001 certification and promises legal and
  accessibility support for VisMo 6 on its [product page](https://webhouse.cz/redakcni-system-vismo-6).
  Certificate scope, validity and fulfillment of a specific contract still need
  verification. This is a vendor statement, not an independent audit of a website.

Compliance depends on the actual service, system classification, contracts,
operational measures and published content. Neither Czech hosting nor ISO 27001
alone certifies all of those aspects. See [deployment.md](deployment.md).

## Requirements for our Rust website

The current deployment expects a Linux host running Docker Compose, with an
Axum/Leptos server, persistent local storage, a reverse proxy and an SMTP service.
The operator must support an application supplied by us and define who maintains
its releases, operating system, backups and incidents.

PostgreSQL 18 is the application database. Use the persistent `postgres-data`
volume in Compose or a managed PostgreSQL service with appropriate TLS, backup
and recovery arrangements. See [database setup](database.md#production-connections-and-recovery).

A practical next step is to obtain quotes for the same scope: managed Linux,
our container image, persistent storage, encrypted offsite backups, monitoring
and recovery. Webhouse or OptoNet would need to confirm support for that workload.
The previously considered CRA/Cloud4com service also needs an exact product and
management scope in its quote. An approved VisMo SaaS offer, if supplied, would
not automatically cover running our application on unrelated infrastructure.

## Information required before selection

1. Exact service and, where applicable, DIA offer identifier and security level.
2. Written support for our Docker deployment and PostgreSQL database and persistent storage.
3. Production, replica, backup and support-access locations and subprocessors.
4. SLA, incident response, recovery time and acceptable data loss.
5. Responsibility for OS updates, application releases and security fixes.
6. Encrypted offsite backup retention, tested recovery and complete data export.
7. SMTP terms, processor agreement, exit procedure and full recurring cost.

These points form a quote specification. They do not authorize procurement or
contacting suppliers on the municipality's behalf.
