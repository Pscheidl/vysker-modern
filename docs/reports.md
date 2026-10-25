# Fault reports

Residents browse **Nahlášené závady** at `/hlaseni-zavad` and open the separate
**Nahlásit závadu** form at `/hlaseni-zavad/nove`. Each accepted report immediately has a public detail at
`/hlaseni-zavad/{id}`. The newest reports appear first. Migration
`0015_issue_reports.sql` adds the **Hlášení závad** navigation entry and creates
the report, recipient-settings and notification-queue tables in PostgreSQL.
Migration `0016_issue_report_photos.sql` moves every existing photograph into
the first position in the new `issue_report_photos` gallery table. Existing
reports and their photographs are preserved.
Migration `0017_issue_report_resolutions.sql` adds public repair notes and their
separate photograph storage without changing the original report gallery.
The list searches descriptions and addresses on the server using `q`, for
example `/hlaseni-zavad?q=lavička`. Search terms remain in the URL so filtered
results can be shared and reloaded. Pagination preserves the filter, and
**Zrušit hledání** returns to the unfiltered list.
List cards show a short excerpt from the first sentence, capped at 120
characters, with `...` when more text follows. The detail retains the complete
submitted description, and searching covers the full description.

The form requires a description and either an address or a map position.
Both can be supplied together. **Kdo nahlásil** and photographs are optional.
Empty names display as anonymous reports. Address suggestions use Photon
through the application's server. Selecting a suggestion fills the address
and places a pin on the map. Clicking the map, moving the pin, selecting the
map centre or using phone location replaces the address field with the exact
coordinates as `latitude, longitude`, each to six decimal places. This keeps
the submitted location aligned with the visible pin. Free text remains
valid without choosing a suggestion, including when the search service is
unavailable. There is no reverse geocoding when moving a pin.

Residents can also type or paste GPS coordinates into the address field. The
form accepts decimal-point pairs separated by a comma, space or semicolon, and
decimal-comma pairs separated by a semicolon. Valid coordinates place a pin
without a Photon query and are normalized when leaving the field. Coordinates
outside the latitude or longitude bounds prevent submission until corrected.
Removing a pin clears a coordinate value from the address field, while a
separately typed street address remains available.

After saving, the form opens the new public detail with the explicit confirmation
**Hlášení bylo odesláno. Děkujeme.** A resident leaving a filled form through a
normal link is warned before their draft is discarded. Cancelling keeps the
text, selected location and photographs. Reloading or closing the page uses
the browser's standard unsaved-changes prompt where supported. Empty forms and
navigation after a successful submission do not prompt. Browser Back and Forward
within the running single-page application preserve the draft in memory,
including the selected files, and show **Rozpracované hlášení bylo obnoveno.**
on return. Confirming departure through a link or successfully submitting clears
that draft. Drafts are not saved to browser storage or the server, so they do
not survive an actual page reload, browser closure or crash.

## Administration and API

Open `/admin/hlaseni-zavad` to configure recipients, inspect reports, view the
private request IP, change the repair status and hide or restore public visibility.
Reports use the states `reported`, `in_progress` and `resolved`. Timestamps are
stored with the Europe/Prague offset applicable when submitted and displayed
in that local time.

| Endpoint | Purpose |
| --- | --- |
| `POST /api/v1/reports` | Create a report using multipart form data |
| `GET /api/v1/reports?limit=21&offset=0&q=lavička` | List public reports, optionally filtering their description or address, newest first |
| `GET /api/v1/reports/map?q=lavička` | Read matching located reports across all list pages, with map counts |
| `GET /api/v1/reports/{id}` | Read one public report |
| `GET /api/v1/reports/{id}/photo` | Read its processed JPEG |
| `GET /api/v1/reports/{id}/photos/{position}` | Read a processed gallery photograph, starting at position 1 |
| `GET /api/v1/report-addresses?q=Vyskeř%2050` | Find address suggestions with labels and coordinates |
| `GET /api/v1/admin/reports?limit=21&offset=0` | List all reports, including private IP and hidden state |
| `PUT /api/v1/admin/reports/{id}` | Change `status`, `hidden`, or both |
| `PUT /api/v1/admin/reports/{id}/resolution` | Save an optional public repair comment, photographs and status using multipart form data |
| `GET /api/v1/admin/reports/{id}/photo` | Read a photograph even when its report is hidden |
| `GET /api/v1/admin/reports/{id}/photos/{position}` | Read any gallery photograph, including hidden reports |
| `GET /api/v1/reports/{id}/resolution-photos/{photo_id}` | Read a public photograph of the repair |
| `GET /api/v1/admin/reports/{id}/resolution-photos/{photo_id}` | Read a repair photograph even when the report is hidden |
| `GET /api/v1/admin/report-settings` | Read notification recipients |
| `PUT /api/v1/admin/report-settings` | Save `{"recipients":["repairs@example.test"]}` |

Public and administrative report objects include `photo_count` and retain
`has_photo`. The singular `/photo` endpoints continue to return the first image
for existing links. The public detail and administration display the complete
gallery, while list cards use its first image.

Administrative endpoints require a session. Mutations also require the current
CSRF token. Report updates accept only supplied fields, so changing a status
preserves a visibility change made by another administrator. An empty update
or an update with only null values is rejected. The administration reads the
complete updated state from the server response. Status and visibility changes,
and changes to recipient settings, are recorded in the existing audit log.

Hiding a report excludes its text and all photographs from public endpoints and
pages. It remains available to administrators. Hiding is not deletion and does
not recall copies already viewed, downloaded or emailed.

## Repair notes and photographs

The administrator can mark a report as resolved without extra information, or
add one editable public comment of up to 5,000 characters and photographs of
the repair. The public detail shows these under **Řešení závady**, separately
from the resident's original text and photographs. Comments and repair photos
can also document work still in progress. The administration offers **Uložit
řešení** and **Uložit a označit jako vyřešené**. Hidden reports hide both galleries
from the public, while administrators retain access.

Report objects include `resolution_comment`, `resolution_updated_at` and
`resolution_photo_ids`. Repair photo IDs are stable. A multipart update accepts
`resolution_comment`, optional `status`, optional `remove_photo_ids` as a JSON
array, and repeated `photo` fields. Omitted fields retain their saved values.
An empty supplied comment clears it. Existing photos remain unless their IDs
are explicitly removed. New photos are appended. The comment, status, removals
and additions are committed together, so failed uploads do not partially
change a repair record.

Repair photographs use the same JPEG, PNG and WebP validation, metadata removal
and per-file 15,000,000-byte limit as resident photographs. A request accepts
up to 50,000,000 bytes of new images. All retained and new processed repair
photographs together must also fit within 50,000,000 bytes. The administration
shows previews and permits individual removal before saving. Failed saves
preserve the comment, new files and proposed removals for retry. Closing the
detail, opening another report, refreshing the list or paging warns before
discarding unsaved changes. The existing administration navigation guard also
covers this form.
If the connection fails after the server commits an upload, the browser may
not receive its confirmation. Check the public detail or administration in a
separate tab before repeating that upload, because repeating it can add the
photographs again.

## Notifications

The recipient list is **empty by default**. With an empty list, reports are
still accepted and published, but no notification is queued. In administration,
enter one email address per line. The server validates at most 20 submitted
addresses, normalizes them and removes duplicates.

Each report and its recipient-specific queue entries are saved in the same
database transaction. The recipients are a snapshot of the settings at submission.
Changing or clearing the list affects new reports, not messages already queued.
Each recipient receives a separate plain-text email containing the description,
supplied name, location and links to the detail and photographs. The request IP
is not included. Photographs are linked, not attached.

Delivery uses the existing SMTP worker and the sender configured under
**Rozesílání → Nastavení odesílání**, or the server's SMTP settings. See
[mail.md](mail.md). It also honors the existing local SMTP capture mode. It
does not depend on newsletter subscriptions or their consent configuration.

The queue persists across restarts, retries failures with increasing delays
up to six hours and reclaims expired five-minute worker leases. SMTP submission
can occur more than once if the process stops after SMTP acceptance but before
the database records success. A stable Message-ID assists deduplication.
`sent_at` means SMTP acceptance, not confirmed inbox delivery. Successful
submission removes the stored email body, while queue metadata remains.
The existing newsletter mail-history screen does not list this separate queue.

## Photo and submission limits

The server checks the actual image bytes, rather than trusting the filename
or browser MIME type. Supported input formats are JPEG, PNG and WebP.

| Limit | Value |
| --- | --- |
| Description | 5,000 characters |
| Address or location description | 300 characters |
| Optional reporter name | 120 characters |
| Each input photograph | 15 MB, exactly 15,000,000 bytes |
| All input photographs together | 50 MB, exactly 50,000,000 bytes |
| All processed photographs together | 50 MB, to bound memory and stored gallery size |
| Entire multipart request | 50,000,000 bytes plus 64 KiB for fields and framing |
| Input image dimensions | At most 8,192 pixels on either side and 32 million pixels total |
| Decoder allocation limit | 128 MiB |
| Processed image dimensions | At most 2,560 pixels on either side |
| Concurrent image decoding | Two jobs per application process |

Photographs are decoded, oriented according to EXIF and encoded again as JPEG
at quality 85. The stored public image contains no original EXIF, embedded GPS,
profiles or other source metadata. The original uploaded file is not retained.
Each processed file must also fit within 15 MB. HEIC and other formats must be
converted before upload.

The file input accepts multiple files at once and residents can add more later.
Each selected file has its own preview and remove button. Removing one keeps
the remaining selection, and a failed submission preserves all form fields,
coordinates and selected photographs for another attempt. The browser checks
the same per-file and combined byte limits before sending. The server enforces
the limits independently. Accepted photographs are stored in selection order.
An invalid new selection is rejected as a whole with a visible error. Previously
selected valid photographs remain available to submit or remove.

Multipart submissions repeat the `photo` field for multiple images. Server
validation rejects unknown or duplicate text fields, incomplete or
out-of-range coordinates, missing descriptions and reports without any location.
The create endpoint requires an Origin matching `OBEC_VEREJNA_URL` exactly.
Per-IP request counters allow five attempts per 15-minute window and 30 per
day-long window. Validation failures after origin checking also count.

## IP addresses and public data

Every accepted report stores the request IP in `issue_reports.request_ip`.
Public API responses, pages and notification emails omit it. The administrator
can see it in the report detail. Public reports expose the description, location,
optional supplied name, processed photographs, timestamp and repair status.

IP extraction uses the actual TCP peer. It accepts `X-Real-IP` only when that
peer exactly equals the configured `OBEC_TRUSTED_PROXY` IP. It never trusts
`X-Forwarded-For`. The trusted proxy must overwrite `X-Real-IP`, and the
application port must not be exposed as an alternative public entry point.
See [operations.md](operations.md) for the existing proxy setup. Internal tests
without peer information use the fallback value `local`.

Report records, their request IPs and notification metadata currently have no
automatic expiry or deletion workflow. Newsletter retention settings do not
apply to these tables. This behavior must be reflected in the operator's report
privacy information and retention procedure. Hiding a report does not remove
its stored personal data or backup copies.

## Maps and phone location

The overview above the public list uses the same `q` filter across all matching
reports, independently of the list's current page. It plots at most the 2,000
newest matching reports with coordinates and explains when that cap is reached.
Counts distinguish reports shown on the map from matches without coordinates,
which remain available in the list. Opening a marker shows its excerpt, date,
status and a link to the complete detail. The popup scrolls on small screens.

Nearby reports are grouped with locally served Leaflet.markercluster 1.5.3
from `public/vendor/leaflet-markercluster-1.5.3`. Its license and verified source
hashes accompany the assets. Clicking a cluster zooms in, and reports at the
same coordinates spread apart so each can be opened separately. Neither cluster
data nor report content is sent to the map tile provider. Map zoom controls
work without triggering the form's unsaved-draft warning.

The form starts at the municipal office, **Vyskeř 50, 512 64 Vyskeř**, at
`50.5295088, 15.158555` with zoom 17. The address input uses that same real
address as its placeholder. The office address was verified against
[the public authority register](https://portal.gov.cz/organy-verejne-moci-interni/obec-vysker-3009),
and its address-point coordinates against Photon / OpenStreetMap on 2026-10-25.

The application serves Leaflet 1.9.4 and its license locally from
`public/vendor/leaflet-1.9.4`. Map tiles are requested directly by the visitor's
browser from `https://tile.openstreetmap.org/{z}/{x}/{y}.png`. The tile provider
receives normal request metadata and requests for the viewed map area. The
report description and photographs are submitted only to this application's API.

Address suggestions come from the free public [Photon demo service](https://github.com/komoot/photon#demo-server),
which uses OpenStreetMap data. Its operator permits reasonable use, offers no
availability guarantee, and may throttle or block excessive traffic. There is
no paid account or API key. The form explains that typed address queries are
sent to this service. Queries go through `/api/v1/report-addresses` rather than
directly from the visitor's browser. The proxy requires at least three characters,
caches up to 256 results for 60 minutes, allows at most one upstream request per
second across the application process and uses a six-second timeout. If public
traffic grows beyond reasonable use, operate a Photon instance or select an
appropriate alternative service.

The address input offers keyboard-accessible suggestions, cancels superseded
searches and ignores late responses. Search errors leave the entered address,
manual pin selection and form submission available.

Keep visible OpenStreetMap contributor attribution linking to the
[OpenStreetMap copyright and license page](https://www.openstreetmap.org/copyright).
OSM data is available under the ODbL, while use of the community tile servers
also follows the [OSMF tile usage policy](https://operations.osmfoundation.org/policies/tiles/).
The map uses normal browser caching and sets a tile-specific referrer policy
that sends the site's origin. Do not replace it with a policy that suppresses
the Referer header, bypass tile caches or add bulk downloads or offline tile
prefetching. OSM's tile service is best effort, without an availability guarantee.
An unavailable map still allows submission using a typed address.

The **Použít moji polohu** button is shown only when browser identification
indicates a mobile phone. A small desktop window alone does not enable it.
Location is requested only after pressing that button, with a 15-second timeout
and no cached position. No continuous location watch is started. The user can
move or remove the resulting pin before submitting.

Browser geolocation needs a secure context and user permission. Production
must use HTTPS, and any upstream proxy must preserve the application's
`Permissions-Policy` allowance `geolocation=(self)`. See
[MDN's getCurrentPosition documentation](https://developer.mozilla.org/en-US/docs/Web/API/Geolocation/getCurrentPosition).
Denied permission, unsupported browsers and location failures leave address
entry and manual pin placement available. Browser device identification can
vary, so exercise the final page on the actual supported phones.

## Verification

The dedicated API tests are in `tests/reports.rs`. They exercise anonymous and
named reports, private IP handling, public hiding, image validation and metadata
removal, recipient settings, transaction rollback, limits, SMTP retry and expired
leases. Browser scenarios are in `tests/browser/reports.spec.ts` and
`tests/browser/reports-admin.spec.ts`. These cover the form, desktop and mobile
location behavior, direct GPS input and coordinate synchronization, address
autocomplete and stale results, unsaved-draft protection and success confirmation, multiple photographs,
per-file removal and retry, exact upload-size boundaries, recipient edits and
moderation, including stale admin views. They also cover filtered map markers,
overlapping reports and mobile popups, plus a real repair save with a comment,
photograph, resolved status, public detail and hiding of repair photographs.

Use the disposable PostgreSQL workflow in [testing.md](testing.md). Run the
Rust tests before rebuilding the complete web assets for browser checks, because
test builds can replace the web binary with one lacking asset settings. After
the build, run the focused browser scenarios with:

```sh
pnpm test:e2e reports.spec.ts reports-admin.spec.ts
```

Browser tests use the local disposable server and synthetic data. Do not run
them against production or rely on them to prove delivery to a real mailbox.
This document describes implementation and test coverage, not a completed
verification run or a deployment.
