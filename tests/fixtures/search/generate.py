#!/usr/bin/env python3
"""Regenerate small deterministic search fixtures. Requires reportlab and Pillow."""
from pathlib import Path
from xml.sax.saxutils import escape
import io
import json
import zipfile

from PIL import Image, ImageDraw, ImageFont
from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.lib.utils import ImageReader

ROOT = Path(__file__).resolve().parent
FONT = '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf'
TEXT = 'Zastupitelstvo schválilo pronájem obecní hospody. Nájemné činí 15000 korun měsíčně.'


def archive(name, files):
    with zipfile.ZipFile(ROOT / name, 'w', zipfile.ZIP_DEFLATED) as out:
        for path, data in files.items():
            entry = zipfile.ZipInfo(path, (2026, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_STORED if path == 'mimetype' else zipfile.ZIP_DEFLATED
            out.writestr(entry, data)


def main():
    ROOT.mkdir(parents=True, exist_ok=True)
    (ROOT / 'obec.txt').write_text(TEXT, encoding='utf-8')
    (ROOT / 'obec.csv').write_text('Předmět;Rozhodnutí;Částka\n"Obecní hospoda";"Schválen pronájem";15000\n', encoding='utf-8')
    (ROOT / 'obec.json').write_text(json.dumps({'predmet': 'Obecní hospoda', 'rozhodnuti': 'Schválen pronájem', 'najemne': 15000}, ensure_ascii=False), encoding='utf-8')
    (ROOT / 'obec.xml').write_text('<?xml version="1.0" encoding="UTF-8"?><usneseni cislo="42"><predmet>Obecní hospoda</predmet><rozhodnuti>Schválen pronájem</rozhodnuti><najemne>15000</najemne></usneseni>', encoding='utf-8')
    content_types = '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>{}</Types>'
    relationships = '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>'
    archive('obec.docx', {
        '[Content_Types].xml': content_types.format('<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'),
        '_rels/.rels': relationships.format('<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'),
        'word/document.xml': '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>' + escape(TEXT) + '</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Poplatek</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Částka</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>Nájemné</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>15000</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:sectPr/></w:body></w:document>',
    })
    ods_table = '<table:table table:name="Hospoda"><table:table-row><table:table-cell office:value-type="string"><text:p>Předmět</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>Částka</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell office:value-type="string"><text:p>Pronájem obecní hospody</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="15000"><text:p>15000</text:p></table:table-cell><table:table-cell table:number-columns-repeated="16382"/></table:table-row><table:table-row table:number-rows-repeated="1048574"><table:table-cell table:number-columns-repeated="16384"/></table:table-row></table:table>'
    for suffix, kind, body in [('odt', 'text', '<text:h text:outline-level="1">Zápis zastupitelstva</text:h><text:p>' + escape(TEXT) + '</text:p>'), ('ods', 'spreadsheet', ods_table)]:
        archive('obec.' + suffix, {
            'mimetype': 'application/vnd.oasis.opendocument.' + kind,
            'META-INF/manifest.xml': '<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.' + kind + '"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>',
            'content.xml': '<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" office:version="1.2"><office:body><office:' + kind + '>' + body + '</office:' + kind + '></office:body></office:document-content>',
        })
    archive('obec.xlsx', {
        '[Content_Types].xml': content_types.format('<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>'),
        '_rels/.rels': relationships.format('<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'),
        'xl/workbook.xml': '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Hospoda" sheetId="1" r:id="rId1"/></sheets></workbook>',
        'xl/_rels/workbook.xml.rels': relationships.format('<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'),
        'xl/sharedStrings.xml': '<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="3" uniqueCount="3"><si><t>Předmět</t></si><si><t>Částka</t></si><si><t>Pronájem obecní hospody</t></si></sst>',
        'xl/worksheets/sheet1.xml': '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>15000</v></c></row></sheetData></worksheet>',
    })
    pdfmetrics.registerFont(TTFont('Fixture', FONT))
    def native_page(pdf):
        pdf.setFont('Fixture', 12)
        pdf.drawString(45, 790, 'Zápis zastupitelstva obce Vyskeř')
        pdf.drawString(45, 755, 'Zastupitelstvo schválilo pronájem obecní hospody.')
        pdf.drawString(45, 730, 'Nájemné činí 15000 korun měsíčně.')
        pdf.showPage()
    image = Image.new('RGB', (1654, 2339), 'white')
    draw = ImageDraw.Draw(image)
    font = ImageFont.truetype(FONT, 38)
    lines = ['Zápis zastupitelstva obce Vyskeř', 'Rozhodnutí o pronájmu obecní hospody', 'Nájemné činí 15000 korun měsíčně.', 'Hlasování: pro 7, proti 0, zdržel se 0.']
    for index, line in enumerate(lines):
        draw.text((100, 130 + index * 85), line, font=font, fill='black')
    # Deterministic image-only page, representative of a clean A4 office scan.
    def scan_page(pdf):
        pdf.drawImage(ImageReader(image), 0, 0, width=595, height=842)
        pdf.showPage()
    def hybrid_page(pdf):
        pdf.drawImage(ImageReader(image), 0, 0, width=595, height=842)
        pdf.setFont('Fixture', 12)
        pdf.drawString(45, 30, 'Elektronický podpis dokumentu a označení obecního úřadu.')
        pdf.showPage()
    for name, functions in [('text.pdf', [native_page]), ('scan.pdf', [scan_page]), ('mixed.pdf', [native_page, scan_page]), ('hybrid.pdf', [hybrid_page])]:
        pdf = canvas.Canvas(str(ROOT / name), pagesize=(595, 842), invariant=1)
        for function in functions:
            function(pdf)
        pdf.save()
    image.thumbnail((600, 850))
    image.save(ROOT / 'scan-preview.png')


if __name__ == '__main__':
    main()
