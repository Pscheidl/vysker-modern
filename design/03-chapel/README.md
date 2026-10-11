# Standalone chapel preview

Frontend illustration approved on 11 October 2026.

The public homepage and municipal information illustration use a standalone
ceramic Saint Anne chapel, without the surrounding village, hill or terrain.
The existing typography and green theme remain in place. The original relief
assets and administration artwork are preserved.

## Assets

- [Light chapel](../../public/images/chapel-ceramic-light.png), 1536 × 1024 RGBA PNG.
- [Dark chapel](../../public/images/chapel-ceramic-dark.png), 1536 × 1024 RGBA PNG.

Both variants were created with the built-in imagegen tool. The light variant
uses the existing light relief as its material reference and the chapel mark as
its architectural reference. The dark variant edits the light chapel with the
old dark relief as a material and lighting reference. This is a stylized image.

Exact prompts: [light](prompt-light.txt), [dark](prompt-dark.txt).

## Local preview

From the project root:

```bash
env -u NO_COLOR target/tools/trunk/trunk serve --release=false --cargo-profile dev --address 127.0.0.1 --port 8080 --watch src --watch style --watch public --watch demo
```

Open <http://127.0.0.1:8080/>. This builds the current frontend with sample data.
No database or backend is needed.

## Visual checks

- The demo build completed successfully.
- The homepage was checked in light and dark themes at desktop width and 375 px.
- Both chapel assets load, the whole building is visible and the page has no
  horizontal overflow at the checked widths.
- The municipal information page was checked with the new illustration.
- Screenshots are in [screenshots/leptos/chapel](../../screenshots/leptos/chapel/).
