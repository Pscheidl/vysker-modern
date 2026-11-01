//! Printable settlement derived from the durable text e-mail snapshot.
//!
//! This writer only creates PDFs. It never parses an uploaded invoice. Font
//! metrics come exclusively from the two trusted fonts compiled into the app.

use std::collections::BTreeMap;

use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use skrifa::Font;

const PAGE_WIDTH: f32 = 595.276;
const PAGE_HEIGHT: f32 = 841.89;
const MARGIN: f32 = 20.0 * 72.0 / 25.4;
const TEXT_WIDTH: f32 = PAGE_WIDTH - 2.0 * MARGIN;
const GREEN: [f32; 3] = [39.0 / 255.0, 109.0 / 255.0, 74.0 / 255.0];
const BLACK: [f32; 3] = [0.0; 3];
const FONT_ERROR: &str = "Nelze načíst písmo vyúčtování";
const PDF_ERROR: &str = "PDF vyúčtování nelze vytvořit";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Weight {
    Regular,
    Bold,
}

impl Weight {
    fn index(self) -> usize {
        match self {
            Self::Regular => 0,
            Self::Bold => 1,
        }
    }

    fn resource(self) -> Name<'static> {
        match self {
            Self::Regular => Name(b"Regular"),
            Self::Bold => Name(b"Bold"),
        }
    }
}

#[derive(Clone, Copy)]
struct Character {
    unicode: char,
    cid: u16,
    glyph: u16,
    /// Advance in PDF font units, where one em is 1000 units.
    width: f32,
}

struct Typeface {
    font: Font,
    bytes: &'static [u8],
    name: &'static [u8],
    scale: f32,
    ascent: f32,
    descent: f32,
    characters: Vec<Character>,
    codes: BTreeMap<char, Character>,
}

impl Typeface {
    fn new(bytes: &'static [u8], name: &'static [u8]) -> Result<Self, String> {
        let font = Font::new(bytes, 0).ok_or(FONT_ERROR)?;
        let metrics = font.metrics();
        if metrics.raw_units_per_em == 0 || metrics.num_glyphs == 0 {
            return Err(FONT_ERROR.to_owned());
        }
        let scale = 1000.0 / f32::from(metrics.units_per_em);
        let line = metrics.h_line().ok_or(FONT_ERROR)?;
        let ascent = line.ascender.to_f32() * scale;
        let descent = line.descender.to_f32() * scale;
        if !ascent.is_finite() || !descent.is_finite() || ascent <= descent {
            return Err(FONT_ERROR.to_owned());
        }
        Ok(Self {
            font,
            bytes,
            name,
            scale,
            ascent,
            descent,
            characters: Vec::new(),
            codes: BTreeMap::new(),
        })
    }

    fn character(&mut self, unicode: char) -> Result<Character, String> {
        if let Some(character) = self.codes.get(&unicode) {
            return Ok(*character);
        }
        let charmap = self.font.charmap();
        // A missing glyph remains visible, and receives its own CID so copying
        // or extracting text still recovers the original Unicode character.
        let glyph = charmap
            .map_unicode(unicode)
            .or_else(|| charmap.map_unicode('\u{fffd}'))
            .or_else(|| charmap.map_unicode('?'))
            .ok_or(FONT_ERROR)?;
        let width = self.font.glyph_metrics().h_advance(glyph) * self.scale;
        if !width.is_finite() || width < 0.0 {
            return Err(FONT_ERROR.to_owned());
        }
        let character = Character {
            unicode,
            cid: u16::try_from(self.characters.len() + 1).map_err(|_| PDF_ERROR)?,
            glyph: u16::try_from(glyph.to_u32()).map_err(|_| FONT_ERROR)?,
            width,
        };
        self.characters.push(character);
        self.codes.insert(unicode, character);
        Ok(character)
    }

    fn embed(&self, pdf: &mut Pdf, ids: &mut ObjectIds) -> Result<Ref, String> {
        let root = ids.next()?;
        let descendant = ids.next()?;
        let descriptor = ids.next()?;
        let file = ids.next()?;
        let unicode = ids.next()?;
        let mapping = ids.next()?;
        let name = Name(self.name);
        let system = SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        };
        pdf.type0_font(root)
            .base_font(name)
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(descendant)
            .to_unicode(unicode);
        let mut cid = pdf.cid_font(descendant);
        cid.subtype(CidFontType::Type2)
            .base_font(name)
            .system_info(system)
            .font_descriptor(descriptor)
            .cid_to_gid_map_stream(mapping)
            .default_width(0.0);
        cid.widths()
            .consecutive(1, self.characters.iter().map(|ch| ch.width));
        cid.finish();

        let metrics = self.font.metrics();
        let bounds = metrics.bounds;
        let cap_height = metrics
            .cap_height
            .map(|v| v.to_f32() * self.scale)
            .unwrap_or(self.ascent);
        pdf.font_descriptor(descriptor)
            .name(name)
            .flags(FontFlags::SYMBOLIC)
            .bbox(Rect::new(
                bounds.x_min.to_f32() * self.scale,
                bounds.y_min.to_f32() * self.scale,
                bounds.x_max.to_f32() * self.scale,
                bounds.y_max.to_f32() * self.scale,
            ))
            .italic_angle(0.0)
            .ascent(self.ascent)
            .descent(self.descent)
            .cap_height(cap_height)
            .stem_v(80.0)
            .font_file2(file);
        pdf.stream(file, self.bytes).pair(
            Name(b"Length1"),
            i32::try_from(self.bytes.len()).map_err(|_| FONT_ERROR)?,
        );

        // CID zero is .notdef. Other CIDs are assigned to Unicode characters,
        // not glyph IDs, because different characters may share a fallback.
        let mut glyphs = vec![0, 0];
        let mut cmap = UnicodeCmap::new(Name(b"RentalUnicode"), system);
        for character in &self.characters {
            glyphs.extend_from_slice(&character.glyph.to_be_bytes());
            // The library encodes supplementary-plane characters as UTF-16
            // surrogate pairs, rather than truncating them to a u16.
            cmap.pair(character.cid, character.unicode);
        }
        pdf.stream(mapping, &glyphs);
        pdf.stream(unicode, &cmap.finish());
        Ok(root)
    }
}

/// Allocate every indirect object once, with checked positive PDF object IDs.
struct ObjectIds(i32);

impl ObjectIds {
    fn next(&mut self) -> Result<Ref, String> {
        let current = self.0;
        self.0 = current.checked_add(1).ok_or(PDF_ERROR)?;
        Ok(Ref::new(current))
    }
}

#[derive(Clone, Copy)]
struct Letter {
    character: Character,
    weight: Weight,
}

struct Line {
    letters: Vec<Letter>,
    baseline: f32,
    size: f32,
    color: [f32; 3],
}

struct Layout {
    fonts: [Typeface; 2],
    pages: Vec<Vec<Line>>,
    top: f32,
}

impl Layout {
    fn new() -> Result<Self, String> {
        Ok(Self {
            fonts: [
                Typeface::new(
                    include_bytes!("assets/LiberationSans-Regular.ttf"),
                    b"LiberationSans",
                )?,
                Typeface::new(
                    include_bytes!("assets/LiberationSans-Bold.ttf"),
                    b"LiberationSans-Bold",
                )?,
            ],
            pages: vec![Vec::new()],
            top: PAGE_HEIGHT - MARGIN,
        })
    }

    fn room_for(&mut self, height: f32) {
        if self.top - height < MARGIN {
            self.pages.push(Vec::new());
            self.top = PAGE_HEIGHT - MARGIN;
        }
    }

    fn gap(&mut self, height: f32) {
        self.top -= height;
    }

    fn paragraph(
        &mut self,
        spans: &[(&str, Weight)],
        size: f32,
        color: [f32; 3],
    ) -> Result<(), String> {
        let mut letters = Vec::new();
        for (text, weight) in spans {
            for character in text.chars() {
                let character = if character.is_whitespace() {
                    ' '
                } else {
                    character
                };
                letters.push(Letter {
                    character: self.fonts[weight.index()].character(character)?,
                    weight: *weight,
                });
            }
        }
        let ascent = self
            .fonts
            .iter()
            .map(|font| font.ascent)
            .fold(0.0_f32, f32::max)
            * size
            / 1000.0;
        let descent = self
            .fonts
            .iter()
            .map(|font| font.descent)
            .fold(0.0_f32, f32::min)
            * size
            / 1000.0;
        let line_height = (size * 1.3).max(ascent - descent);
        let mut start = 0;
        while start < letters.len() {
            while start < letters.len() && letters[start].character.unicode == ' ' {
                start += 1;
            }
            if start == letters.len() {
                break;
            }
            let mut end = start;
            let mut width = 0.0;
            let mut break_at = None;
            // Also bound zero-width runs so the PDF string stays reasonably
            // small even for an unusual input made entirely of combining marks.
            while end < letters.len() && end - start < 4096 {
                let advance = letters[end].character.width * size / 1000.0;
                if width + advance > TEXT_WIDTH {
                    break;
                }
                width += advance;
                if letters[end].character.unicode == ' ' {
                    break_at = Some(end + 1);
                }
                end += 1;
            }
            if end == start {
                return Err(PDF_ERROR.to_owned());
            }
            if end < letters.len() {
                if let Some(at) = break_at {
                    end = at;
                }
            }
            let mut visible_end = end;
            while visible_end > start && letters[visible_end - 1].character.unicode == ' ' {
                visible_end -= 1;
            }
            self.room_for(line_height);
            self.pages.last_mut().ok_or(PDF_ERROR)?.push(Line {
                letters: letters[start..visible_end].to_vec(),
                baseline: self.top - ascent,
                size,
                color,
            });
            self.top -= line_height;
            start = end;
        }
        Ok(())
    }

    fn text(
        &mut self,
        text: &str,
        weight: Weight,
        size: f32,
        color: [f32; 3],
    ) -> Result<(), String> {
        self.paragraph(&[(text, weight)], size, color)
    }

    fn detail(&mut self, text: &str) -> Result<(), String> {
        if let Some((label, value)) = text.split_once(": ") {
            self.paragraph(
                &[
                    (&format!("{label}: "), Weight::Bold),
                    (value, Weight::Regular),
                ],
                11.0,
                BLACK,
            )
        } else {
            self.text(text, Weight::Regular, 11.0, BLACK)
        }
    }

    fn section(&mut self, text: &str) -> Result<(), String> {
        // Keep a section title with at least the first following detail line.
        self.room_for(33.0);
        self.text(text, Weight::Bold, 10.0, GREEN)?;
        self.gap(4.0);
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, String> {
        let mut pdf = Pdf::new();
        let mut ids = ObjectIds(1);
        let catalog = ids.next()?;
        let tree = ids.next()?;
        let info = ids.next()?;
        pdf.catalog(catalog).pages(tree).lang(TextStr("cs-CZ"));
        pdf.document_info(info)
            .title(TextStr("Vyúčtování pronájmu"))
            .author(TextStr("Obec Vyskeř"));
        let regular = self.fonts[0].embed(&mut pdf, &mut ids)?;
        let bold = self.fonts[1].embed(&mut pdf, &mut ids)?;
        let mut pages = Vec::new();
        for lines in &self.pages {
            let page_id = ids.next()?;
            let content_id = ids.next()?;
            pages.push(page_id);
            let mut page = pdf.page(page_id);
            page.parent(tree)
                .media_box(Rect::new(0.0, 0.0, PAGE_WIDTH, PAGE_HEIGHT))
                .contents(content_id);
            page.resources()
                .fonts()
                .pair(Weight::Regular.resource(), regular)
                .pair(Weight::Bold.resource(), bold);
            page.finish();
            let mut content = Content::new();
            for line in lines {
                let [red, green, blue] = line.color;
                content.set_fill_rgb(red, green, blue).begin_text();
                content.set_text_matrix([1.0, 0.0, 0.0, 1.0, MARGIN, line.baseline]);
                let mut start = 0;
                while start < line.letters.len() {
                    let weight = line.letters[start].weight;
                    let mut end = start + 1;
                    while end < line.letters.len() && line.letters[end].weight == weight {
                        end += 1;
                    }
                    let mut encoded = Vec::with_capacity((end - start) * 2);
                    for letter in &line.letters[start..end] {
                        encoded.extend_from_slice(&letter.character.cid.to_be_bytes());
                    }
                    // All PDF escaping is handled by the writer. User text is
                    // never interpolated into PDF operators or dictionaries.
                    content
                        .set_font(weight.resource(), line.size)
                        .show(Str(&encoded));
                    start = end;
                }
                content.end_text();
            }
            pdf.stream(content_id, &content.finish());
        }
        pdf.pages(tree)
            .count(i32::try_from(pages.len()).map_err(|_| PDF_ERROR)?)
            .kids(pages);
        Ok(pdf.finish())
    }
}

/// Generate a printable settlement. This is not labelled an invoice or tax
/// document because the application does not collect all such details.
pub(super) fn render(body: &str) -> Result<Vec<u8>, String> {
    let summary = body
        .split("\n\n")
        .find(|block| block.starts_with("Vyúčtování pronájmu č. "))
        .ok_or_else(|| "Ve zprávě chybí údaje pro vyúčtování".to_owned())?;
    let payment = body
        .split("\n\n")
        .find(|block| block.starts_with("Platební údaje\n"));
    let no_payment = body
        .split("\n\n")
        .find(|block| block.starts_with("Za tento pronájem není třeba"));
    let mut doc = Layout::new()?;
    doc.text("OBEC VYSKEŘ", Weight::Bold, 11.0, GREEN)?;
    doc.gap(8.0);
    let mut lines = summary.lines();
    let heading = lines.next().ok_or("Ve zprávě chybí číslo vyúčtování")?;
    doc.text(heading, Weight::Bold, 21.0, BLACK)?;
    doc.gap(12.0);
    doc.section("PŘEHLED PRONÁJMU")?;
    for line in lines {
        doc.detail(line)?;
    }
    doc.gap(15.0);
    if let Some(payment) = payment {
        doc.section("PLATEBNÍ ÚDAJE")?;
        for line in payment.lines().skip(1) {
            if !line.starts_with("QR kód pro platbu") {
                doc.detail(line)?;
            }
        }
        doc.gap(9.0);
        doc.text(
            "QR kód pro platbu najdete v přiloženém e-mailu.",
            Weight::Regular,
            11.0,
            BLACK,
        )?;
    } else if let Some(no_payment) = no_payment {
        doc.text(no_payment, Weight::Regular, 11.0, BLACK)?;
    }
    doc.gap(18.0);
    doc.text(
        "V případě dotazů se prosím obraťte na obecní úřad.",
        Weight::Regular,
        11.0,
        BLACK,
    )?;
    doc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Dobrý den,\n\nVyúčtování pronájmu č. 42\nNájemce: Jan Novák\nPředmět: Obecní sál\nTermín: 1. 11. 2026, 18:00 až 19:00\nNájemné: 800 Kč\nEnergie: 200 Kč\nCelkem k úhradě: 1000 Kč\n\nPlatební údaje\nČíslo účtu: 19-2000145399/0800\nVariabilní symbol: 42\nSplatnost: 8. 11. 2026\nQR kód pro platbu najdete v této zprávě.";

    #[test]
    fn rejects_missing_summary() {
        assert!(render("Dobrý den").is_err());
    }

    #[test]
    fn makes_pdf_with_czech_text() {
        let pdf = render(SAMPLE).expect("PDF");
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.len() > 1000);
        // Used only by explicit local visual QA, never by the production path.
        if let Some(path) = std::env::var_os("RENTAL_PDF_PREVIEW_PATH") {
            std::fs::write(path, &pdf).expect("write synthetic PDF preview");
        }
    }

    #[test]
    fn unicode_mapping_preserves_czech_and_supplementary_characters() {
        let mut font = Typeface::new(
            include_bytes!("assets/LiberationSans-Regular.ttf"),
            b"LiberationSans",
        )
        .unwrap();
        let input = "Příliš žluťoučký kůň úpěl ďábelské ódy. ŘŠČŤŽĚÁÍÉ 𐀀😀";
        for ch in input.chars() {
            font.character(ch).unwrap();
        }
        let mut pdf = Pdf::new();
        font.embed(&mut pdf, &mut ObjectIds(1)).unwrap();
        let bytes = pdf.finish();
        let text = String::from_utf8_lossy(&bytes);
        for ch in input.chars() {
            let cid = font.codes[&ch].cid;
            let utf16 = ch
                .encode_utf16(&mut [0; 2])
                .iter()
                .map(|unit| format!("{unit:04X}"))
                .collect::<String>();
            assert!(
                text.contains(&format!("<{cid:04X}> <{utf16}>")),
                "missing mapping for {ch}"
            );
        }
        assert_ne!(font.codes[&'𐀀'].cid, font.codes[&'😀'].cid);
    }

    #[test]
    fn long_unbroken_words_paginate_without_losing_text_or_overflowing() {
        let mut layout = Layout::new().unwrap();
        let input = "PřílišŽluťoučkýKůň".repeat(500);
        layout.text(&input, Weight::Regular, 11.0, BLACK).unwrap();
        assert!(layout.pages.len() >= 2);
        let recovered: String = layout
            .pages
            .iter()
            .flatten()
            .flat_map(|line| line.letters.iter().map(|letter| letter.character.unicode))
            .collect();
        assert_eq!(recovered, input);
        for line in layout.pages.iter().flatten() {
            let width: f32 = line
                .letters
                .iter()
                .map(|letter| letter.character.width * line.size / 1000.0)
                .sum();
            assert!(width <= TEXT_WIDTH + 0.01);
            assert!(
                line.baseline + layout.fonts[0].ascent * line.size / 1000.0
                    <= PAGE_HEIGHT - MARGIN + 0.01
            );
            assert!(line.baseline + layout.fonts[0].descent * line.size / 1000.0 >= MARGIN - 0.01);
        }
        let pages = layout.pages.len();
        let pdf = layout.finish().unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains(&format!("/Count {pages}")));
    }

    #[test]
    fn pdf_operator_characters_are_written_as_text() {
        let body = SAMPLE.replace(
            "Jan Novák",
            "Jan (Novák) \\ </Text> /OpenAction /JavaScript",
        );
        let pdf = render(&body).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(!text.contains("/OpenAction"));
        assert!(!text.contains("/JavaScript"));
    }
}
