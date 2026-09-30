# Design 01: landscape relief

Homepage concept for the municipality of Vyskeř, dated 29 September 2026.

- [Light theme](svetly.png)
- [Dark theme](tmavy.png)

## Direction

Prominent typography, neutral surfaces, a violet accent and a sculptural landscape
relief. Primary routes lead to the notice board, documents, office hours and local
services. The homepage presents three recent documents with withdrawal dates and
offers notifications after email verification.

The relief is an illustration, not a verified model of the actual municipality.
Document titles, dates and events in the mockups are sample content.

## References

- [Linear: A calmer interface for a product in motion, 12 March 2026](https://linear.app/now/behind-the-latest-design-refresh).
  Inspiration for reduced visual noise, subtle surface boundaries and neutral colours.
- [Vercel: Geist](https://vercel.com/geist/introduction).
  Inspiration for typography and grid structure.

This is an original interpretation with an added sculptural identity. It does
not use components from those products.

## Creation and implementation

The mockups were generated with the built-in imagegen tool. The light theme was
created first, then edited into the dark theme while preserving content and layout.

Original prompts: [light theme](prompt-svetly.txt), [dark theme](prompt-tmavy.txt).

Status: approved and implemented with Leptos and Axum. The website includes a
mobile layout, both themes and internal pages. The original isolated reliefs
were stored as `public/images/relief-light.png` and `public/images/relief-dark.png`.
The current website uses the [revised chapel and Hůra relief](../02-vysker-relief/README.md)
in `public/images/relief-vysker-light.png` and `public/images/relief-vysker-dark.png`.
Long sample document titles were preserved when implementing the mockup.

See the [project README](../../README.md) for setup and current limitations.
