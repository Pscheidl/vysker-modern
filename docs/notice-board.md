# Official notice board

## Publication rules

Categories help visitors navigate. The legal publication rule is selected separately.

| Rule | Application behavior |
| --- | --- |
| Informational notice | Defaults to 15 days with a manual override, without claiming a statutory minimum |
| Service by public notice under Section 25 | Publication day is day 0. The fifteenth day is included, with a weekend or Czech public holiday moved to the next working day. Withdrawal starts no earlier than the following day |
| Municipal property disposal intention | Conservatively requires 15 complete calendar days after publication day before the decision |
| Council meeting announcement | Conservatively requires seven complete calendar days after publication day before the meeting, with visibility through the meeting day |
| Other legal rule | The responsible person supplies a legal basis and the earliest permitted withdrawal date |

These profiles do not determine which law applies to a particular document.
Special rules or circumstances may require another profile, later withdrawal or
publication without an end date. A manually entered date or interval cannot be
shorter than the calculated minimum. `withdraw_on` is **the first day the notice
is no longer published**, in the Europe/Prague time zone.

References: [SÚKL on Section 25 and time limits](https://sukl.gov.cz/probihajici-a-planovana-rizeni/informace-o-dorucovani-verejnou-vyhlaskou/),
[Ministry of the Interior on municipal property and meetings](https://mv.gov.cz/povinnosti-pri-nakladani-s-majetkem-obce).

## Review before publication

In production, an administrator must specify the rule, legal basis and internal
reference to the stored official original, and confirm review of content,
personal data and attachment accessibility. The website cannot assess the
lawfulness of a document or the accessibility of a particular PDF. Internal
review information remains available only to authorized administrators.

Publication cannot be backdated. Published and scheduled notices are immutable.
Corrections require a new notice. A future date schedules publication, while
`published_at` records the actual publication time. If the server misses the
scheduled day, it returns a still-valid notice to draft and records
`schedule_missed`. A schedule whose entire interval has expired is archived
without sending notifications or claiming that publication actually occurred.

Withdrawal before the minimum requires an explicit emergency action with a
reason, for example a personal data disclosure. This does not establish that
the statutory publication period was satisfied. The responsible administrator
must decide what happens next, including any repeat publication. Production
also requires a reason for ordinary manual withdrawal.

## Archive and attachments

The notice record is never deleted. By default, the public archive displays
only its identifier, category and dates. An optional archive title must contain
no personal data. The original description, reference number, issuer and names
of unavailable attachments are no longer public. This projection applies to
both the API and SSR immediately at expiry, before the next maintenance run.

Attachment contents are removed from the web database on withdrawal by default.
Keeping them in the public archive requires a basis and an end date. Access ends
on that date and maintenance removes the contents. Legacy notices without
supported archive settings are not treated as permission to keep publishing
files. Review active notices and justified archive publication before migration.

Official originals must be stored in the municipal records management system
before publication. The website database and its backups do not replace that
system. A pre-migration backup is also necessary because cleanup may remove
legacy archive attachments.

Public responses containing official notices use `no-store` and `X-Robots-Tag`
to restrict caching and indexing. They cannot retract copies already downloaded.
Reference: [ÚOOÚ on service through official notice boards](https://uoou.gov.cz/poznatek-z-dozorove-cinnosti-k-dorucovani-pisemnosti-na-uredni-desce).

## Publication evidence and outages

Administration can download JSON evidence through
`GET /api/v1/admin/notices/{id}/evidence`. It includes the current internal record,
actual transition times, published record fingerprints and attachment SHA-256
hashes. Events are protected against modification and deletion during the approved
`retention.notice_internal_days` period after withdrawal. Controlled cleanup then
removes them and the internal text. Evidence contains no copies of the files.
Attach it to the official case record after withdrawal under municipal procedures.

The incident form records UTC times and a description of the response. The API
is `POST /api/v1/admin/notices/{id}/incidents` with RFC 3339 `started_at` and
`ended_at` values and a `reason`. Do not include personal data in the reason.
Events are not cryptographically signed and do not independently prove continuous
internet availability or the state of the physical notice board. Independent
monitoring and municipal records cover those aspects.

After an outage, the responsible person assesses delivery effects, any need to
repeat publication and consistency with the physical board. The application
does not silently extend the publication interval.
