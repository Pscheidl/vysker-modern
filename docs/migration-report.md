# Migration rehearsal, 30 September 2026

This report records a local rehearsal against the public Vyskeř website. It does
not approve the content for publication or record a production domain switch.

## Captured and imported

| Item | Result |
| --- | ---: |
| Discovered source page identities | 405 |
| Successfully captured pages | 404 |
| Imported editable content pages | 168 |
| Imported structured calendar events | 236 |
| Imported files, including photographs | 1,139 |
| File bytes | 494,420,693 |
| Imported provenance records | 1,543 |
| Structured active notices found | 0 |
| Records inserted by a repeat import | 0 |
| Notification messages queued | 0 |

Every file was downloaded from the local preview and matched the imported size
and SHA-256. All 1,542 imported historical paths other than the root homepage
returned the expected HTTP 301. The root remains the new application's homepage.
Unknown original event times and end dates are identified explicitly in the UI.

The source bundle and detailed reports are stored under ignored `data/migration/`.
The persistent rehearsal database is separate from the ordinary developer database.
The local preview listens on port 3012. It uses local SMTP configuration and has
not sent subscriber notifications.

## Exceptions requiring content review

- The original `/jiri-jirman/o-1009` link returns HTTP 404. It remains the single
  unresolved internal link and is listed in the capture and import reports.
- Seven source pages are empty: `ms:23`, `ms:400`, `ms:401`, `ms:403`, `ms:1040`,
  `ms:1049` and `ms:1831`. Their records are retained with review warnings.
- Seventy fragment-link occurrences need review because old in-page anchors do
  not survive conversion to restricted Markdown. Links resolve to the containing
  imported page rather than an invented anchor.
- Ordinary files, including archive documents and photographs, are imported as
  documents with attachments. Review their categories and publication suitability.
- No active notice was available to rehearse a real active-notice transfer. The
  notice importer is covered with synthetic tests and leaves notices in draft.
- Review navigation, original test entries, formatting and accessibility against
  actual municipal needs. Source capture is not an editorial approval.

The capture is deliberately marked incomplete because of the original 404.
The local rehearsal used `--allow-incomplete` after inspecting this failure.

## Verification performed

- 84 Rust tests passed, including the two Mailpit integration checks
- 16 Python tests passed, including 10 extraction and PostgreSQL import tests
- 12 Playwright workflow tests passed
- Production smoke test passed for startup gates, pages, headers and JS/WASM/CSS
- Nix shell provided Beautiful Soup, psycopg and Europe/Prague timezone data
- Five actual imported/public pages checked in Chromium at desktop and 375 px
  mobile widths, without page errors, broken images or horizontal overflow
- Calendar screenshot reviewed for the missing-time and missing-end-date labels
- Full HTTP redirect and file checksum verification completed without failures

The gallery URL regression was fixed and retested before the final HTTP check.
For repeatable commands and final acceptance steps, see [migration.md](migration.md).
