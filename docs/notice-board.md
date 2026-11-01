# Official notice board

## Publication rules

Categories help visitors navigate. The legal publication rule is selected separately.

| Rule | Application behavior |
| --- | --- |
| Informational notice | The operator supplies the intended interval or leaves the plan open, without a claimed statutory minimum |
| Service by public notice under Section 25 | Publication day is day 0. The fifteenth day is included, with a weekend or Czech public holiday moved to the next working day. Withdrawal starts no earlier than the following day |
| Municipal property disposal intention | Conservatively requires 15 complete calendar days after publication day before the decision |
| Council meeting announcement | Conservatively requires seven complete calendar days after publication day before the meeting, with visibility through the meeting day |
| Other legal rule | The responsible person supplies a legal basis and the earliest permitted withdrawal date |

These profiles do not determine which law applies to a particular document.
Special rules or circumstances may require another profile, later withdrawal or
publication without an end date. A manually entered date or interval cannot be
shorter than the calculated minimum. `withdraw_on` is the planned first day outside the posting interval in the
Europe/Prague time zone. Actual removal is recorded separately.

References: [SÚKL on Section 25 and time limits](https://sukl.gov.cz/probihajici-a-planovana-rizeni/informace-o-dorucovani-verejnou-vyhlaskou/),
[Ministry of the Interior on municipal property and meetings](https://mv.gov.cz/povinnosti-pri-nakladani-s-majetkem-obce).

## One document and distinct posting events

`documents` is the canonical content table for ordinary records and official
notice-board records. `document_postings` stores each posting with its source,
certainty, precise content fingerprint, planned removal and actual removal.
The `notices` compatibility view is only an adapter over this same content table.
It is not a second content store.

The public **Dokumenty** catalogue contains every currently public document.
**Úřední deska → Aktuálně vyvěšeno** contains documents with an actual current
posting. **Veřejný archiv** contains removed postings whose document metadata
remains public. A retained record may have private attachments, according to the
explicit decision made for that posting. A wholly hidden document is absent from
both views and from public search. All views link to `/dokumenty/{id}`.

A passed planned date alone does not prove an actual removal. The worker records
the removal transaction and then updates the current-state projection. Public
posting history distinguishes the plan and the recorded event. Missing source
dates stay missing. A source day is not expanded into an invented event time.
Actual event timestamps are stored consistently and presented in Europe/Prague.

## Review and permanent content locking

In production, the administrator specifies the rule, legal basis and internal
reference to the stored official original, and confirms review of content,
personal data and attachment accessibility. These internal details are not part
of the public document. The application cannot determine whether the underlying
publication is legally sufficient.

Choose **Zkontrolovat finální náhled** and review the exact saved title, text,
reference number, issuer and ordered files. The warning is **Po vyvěšení již
obsah dokumentu nelze upravit.** The submitted token binds approval to these
content bytes and metadata. Concurrent edits or uploads require a new preview.
The shared write transaction and database guards prevent a publication/edit race.

The first actual posting permanently locks the content and the complete ordered
attachment list. Administrative APIs, imports and maintenance cannot modify,
add, remove or replace those attachments or unlock the document. Withdrawal,
hiding or another posting does not change this lock. The database retains the
SHA-256 fingerprints and stored bytes under stable attachment identifiers.

A future local date schedules the reviewed content. Cancelling a schedule records
its reason, keeps attachments and refuses cancellation after actual posting.
If the server misses a scheduled date it must not manufacture an earlier actual
event or silently satisfy a legal period. Review the recorded missed schedule.

An anonymized version selected for later release has its own final preview and
confirmation token. Confirmation freezes the approved copy before it can be
released after removal. Its administrative state is **Uzamčeno**, without an
invented posting event or a claim that its bytes were originally posted.

Corrections, supplements, replacement documents and anonymized versions are new
linked concepts with new IDs and their own attachment records. They are not
automatically public or posted. Reposting an unchanged locked document adds a new
posting event. Neither creating a correction nor importing history starts a new
legal period by itself.

## Mandatory attachment availability decision

There is no preselected post-removal access policy. The operator must explicitly
choose **Po sejmutí ponechat přílohy veřejné** or **Po sejmutí přílohy pouze pro
administrátory**, or choose a separately linked anonymized version. The draft
notice form and the final posting form require this choice.

Admin-only access after removal hides filenames and downloads from the public
detail and all public file routes. The immutable original bytes remain available
to authenticated administrators. Public access to the whole document is a
separate operation. **Ukončit veřejné zpřístupnění** hides the detail, search
result, public APIs and every direct or legacy attachment link and records an
internal reason. Importers cannot reverse that local decision.

This access choice is independent of content locking. Routine removal does not
delete posted bytes. Retention and disposal remain separate controlled processes
with their own approved policy. The editorial workflow neither promises
unlimited storage nor provides a bypass for modifying a posted original.
Municipal originals must also be retained in the appropriate records system.
Database backups include the web's retained attachments.

Download authorization checks current database access on every request and does
not depend on search-index refresh. File and document responses use no-store
cache controls. Access revocation cannot retract copies already downloaded.

## Imported history and evidence

Imported publication dates, local import time, last source observation and local
posting are separate facts. Historical takeover does not create a local posting
or announce old records as new publications. Documented source postings lock the
accepted content. Unclear history blocks content changes for review without
claiming that a posting has been proved.

Where only today's captured copy is available, the history records capture at
migration and does not claim that its bytes are verified as the original
historically posted version. Hashes detect byte changes but do not prove when a
file was published or continuous availability.

A changed locked source produces a private revision. Acceptance creates a new
linked concept, while the old document and files stay intact. An unchanged
repeat import creates no new record, posting, conflict or announcement. Missing
source items and failed downloads are not deletion or removal instructions.

**Zobrazit historii a interní audit** shows original operational events and
subsequent plan changes. Corrections to evidence dates are appended with old and
new values. Public history shows the original date and a separate correction
note, while audit reasons remain private. The older notice evidence
endpoint remains available for historical incident evidence:
`GET /api/v1/admin/notices/{id}/evidence`.

The incident endpoint accepts UTC RFC3339 `started_at`, `ended_at` and an
internal reason at `POST /api/v1/admin/notices/{id}/incidents`. Recording an
incident does not change the posting period. Responsible staff must assess
its consequences using the municipal records and independent monitoring.

Application events, hashes, successful imports and tests do not prove legally
correct posting on the physical official notice board. They are technical
records of the application's actions and the source evidence available to it.
