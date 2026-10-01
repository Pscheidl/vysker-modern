# Editorial administration

The full server application provides the following workflows. GitHub Pages is a
sample frontend and does not expose the administration or store changes.
All administrators currently have equal privileges. Production administration,
including password recovery, remains restricted to the configured office or VPN
networks by the reverse proxy.

## Password recovery

Choose **Zapomenuté heslo** on the login screen. Enter the administrator email,
then open the link in the message and enter the new password twice. The response
does not reveal whether an active account exists. Recovery never reactivates a
disabled account and does not log the person in automatically.

Links expire after 30 minutes and work once. Tokens contain 256 random bits and
only their SHA-256 hashes are stored in `password_resets`. The queued email holds
the link until SMTP acceptance or cancellation, then its body is removed. The
browser receives the token in a URL fragment, removes it from the address bar
and submits it only when the form is confirmed. Opening a link does not change a
password. Recovery requests and resets are rate limited.

A successful password change or recovery invalidates all account sessions and
outstanding recovery links. Deactivation also revokes recovery links. Passwords
use Argon2id with a fresh random salt. `OBEC_MIN_PASSWORD_LENGTH` controls the
minimum number of Unicode characters, default **24**. The separate maximum is
1024 UTF-8 bytes. The same policy applies to UI, API, operator CLI and recovery.
Increasing the minimum does not invalidate existing passwords at login.

Recovery messages have a dedicated durable queue, `recovery_mail`, processed by
the same SMTP worker. It retries failures, uses a stable Message-ID and supports
concurrent workers through PostgreSQL leases. Message metadata is kept for 30
days and appears in **Rozesílání**. Subscription privacy configuration is not
required for security email. Database restore removes recovery tokens and mail
so a stale backup cannot reactivate a link or resend a recovery message.

Operator recovery remains available when SMTP is unavailable. See [backend.md](backend.md).

## Pages and revisions

Open **Stránky**, create or open a page, and edit its title, address and body.
Use Markdown for headings, emphasis, lists, links, quotes and code. For example:

```markdown
## Office hours

**Monday:** 08:00 to 12:00

- [Contact the municipality](/kontakt)
- [External information](https://example.cz)
```

The formatting buttons insert Markdown at the cursor or wrap selected text.
**Nápověda k Markdownu** includes examples, including tables and strikethrough.
**Náhled textu** uses exactly the same renderer as public pages. Raw HTML is shown
as text. Remote embedded images are disabled, so page content cannot load tracking
pixels. Verified migrated images from `/api/v1/legacy-media/{id}` are allowed.
Uploaded page images from `/api/v1/page-images/{id}` are also allowed.
Links allow HTTP, HTTPS, mailto, tel, local absolute paths and fragments.
Other schemes and protocol-relative addresses are neutralized. A body-level H1
is rendered as H2 because the page title already supplies H1.

### Uploading page images

Save a new page as a draft first. In **Obrázky stránky**, enter a meaningful
**Popis obrázku**, choose a PNG, JPEG, WebP or GIF, then choose **Nahrát a vložit
obrázek**. The editor inserts the image at the cursor and preserves unsaved text.
Save the page to store the inserted Markdown. Use **Dříve nahrané obrázky** to
insert an existing image again with a new description. Remove its Markdown from
the body to remove an image from the page.

Each file may be at most 10 MiB, 8192 × 8192 pixels and 128 MiB when decoded.
The server validates decoded image contents, determines the MIME type itself,
and rejects active formats such as SVG. A page can store up to 100 images.
Upload requests require an administrator session and CSRF protection.

Images remain private until their owning page is published and its saved body
contains the image. Hiding the page or removing the image from its saved body
revokes anonymous access. Administrators can preview drafts. Responses are not
cached. Image bytes and metadata live in PostgreSQL and are included in database
backups. Files are retained for restoring older revisions, even when removed
from the current text. Page image URLs should be reused within their owning page.

Save with **Zveřejnit na webu** checked to publish, or unchecked to keep a draft.
Every save stores an immutable revision with author, timestamp and version.
The history is paginated. **Načíst do editoru** copies an older title and body
into the editor for review. Save to create a new revision. This preserves the
current address and publication choice until the editor explicitly changes them.

Updates must supply `expected_version`. If another editor has saved a newer
version, the API returns HTTP 409 and the form preserves the unsaved text. Open
the current page in another tab, compare, and reconcile before saving. Do not
blindly replace a version number to bypass the conflict. Required production
pages cannot be hidden or renamed.

## Navigation and categories

**Navigace** manages menu labels, local destinations, visibility and numeric
order. Smaller order values appear first. Custom destinations must identify an
existing page. A visible link requires a published page. Hiding a page hides its
menu link, and renaming its slug updates matching menu links atomically. The
site logo, contact access, search, footer and required links remain available
independently of menu configuration.

**Kategorie** manages notice-board categories and their order. Used categories
cannot be deleted because published and archived notices still refer to them.
Names and order remain editable. Edits use version checks and all mutations are
audited. Navigation supports up to 20 entries. Keep the main menu concise.

## Calendar

**Kalendář** provides drafts, publication, event dates, locations, descriptions,
cancellation and edit conflict detection. Times are interpreted in
`Europe/Prague`. The form can accept an explicit RFC3339 UTC offset when a local
time is ambiguous during the autumn clock change. Nonexistent spring times are
rejected. An end time cannot precede its start.

Published upcoming and ongoing events appear on the public calendar and homepage.
Past events have a separate paginated archive. Cancelled events remain visible
with their cancellation label. Each published event has a stable detail URL. Prefer cancellation to deletion
when visitors need to know that an announced event will not take place.
Drafts are not public. Introductory calendar text remains editable as the
`kalendar` content page.

## Subscribers

**Odběratelé** supports literal email search, status filters and pagination.
An active subscriber must have a verified address and current confirmed consent.
The administration cannot activate a subscriber or bypass email confirmation.

Open **Důkazy a správa** to inspect or download a private JSON evidence export.
It includes the address, consent timestamps, exact historical notice snapshots,
retained mail status and relevant audit history. Tokens and message bodies are
excluded. Export access is audited. Handle downloaded personal data according to
the municipality's approved process and retention policy.

Withdrawal and erasure require the administrator's current password, session
and CSRF token. Withdrawal records the consent withdrawal, revokes links and
cancels unsent messages. A message already handed to SMTP cannot be recalled.
Erasure deletes the identity and associated consent, token and mail rows. The
audit retains only the numeric record reference, not a copy of the erased email.
If SMTP owns an active delivery lease, erasure returns a conflict with a retry
time. Withdraw immediately, then retry erasure after the lease ends.

Use the approved subscriber request process to decide when withdrawal or erasure
is appropriate. Restore procedures deliberately discard all subscriptions to
avoid reactivating withdrawn addresses from an older backup.
