# Vyskeř: chapel, relief and identity

Revision of the approved ceramic illustration style, dated 29 September 2026.

## Final assets

- [Light relief](../../public/images/relief-vysker-light.png), 2172 × 724 px, transparent PNG.
- [Dark relief](../../public/images/relief-vysker-dark.png), 2171 × 724 px, transparent PNG.
- [Chapel mark](../../public/images/vysker-mark.svg), standalone SVG.
- [Favicon](../../public/favicon.svg), simplified mark on a violet background.

The header mark uses the same strokes as the standalone SVG and inherits its
colour from the current theme. The reliefs appear on the homepage and in the
municipal information preview.

## Changes

- Saint Anne's Chapel has a low nave, a polygonal roof end and an onion-shaped
  turret topped with a cross.
- Hůra is a distinct hill, with village buildings and a larger church at its foot.
- A violet path connects the village to the chapel.
- **Hůra must remain entirely treeless. Trees belong only in the village.**
  This is an explicit user requirement and takes precedence over woodland shown
  in reference photographs.
- White and graphite ceramic materials, contour layers and transparent backgrounds
  are retained.

This is a stylized illustration based on photographs, not a surveyed terrain
model. Individual buildings and contour lines are simplified for the composition.
The mark is a website identity proposal, not the municipality's official coat of arms.

## References

- [Czech Paradise Association: Saint Anne's Chapel, Vyskeř](https://www.cesky-raj.info/vysker-kaple).
  Description of the elongated octagonal plan, roof and Stations of the Cross.
- [National Heritage Institute: Vyskeř, Saint Anne's Chapel](https://www.npu.cz/uop/liberec/akce-zpravy-edice/prezentace-pamatek/2015/letak-vysker_cz.pdf).
  Chapel photographs, hill profile and a map of the village and Hůra.
- [Lukáš Kalista: chapel from the southwest, Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Vysker_kaple_sv_anny_od_jz.jpg), CC BY-SA 4.0.
  Reference for proportions and the onion-shaped turret.
- [Wander Book: Vyskeř](https://en.wander-book.com/vysker-m64.htm), photograph by Rudolf Ropek.
  Reference for the relationship between the hill, buildings and lower church.

Photographs informed the architecture and geography. They are not included in the
website's public assets. Materials and illustration style follow the approved concept.

## Creation

Raster reliefs were produced with the built-in imagegen tool. The SVG mark and
favicon were edited directly as vector assets.

Original prompts, in order:

1. [Architecture and geography revision](prompt-01-geografie.txt).
2. [Removing trees outside the village following user feedback](prompt-02-bez-stromu.txt).
3. [Dark theme of the same relief](prompt-03-tmavy.txt).

The intermediate version with trees on Hůra is not deployed. The final assets
follow the clarification in the second prompt.
