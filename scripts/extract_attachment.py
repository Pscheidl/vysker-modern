#!/usr/bin/env python3
"""Extract searchable attachment text in a bounded, isolated subprocess.

Only the JSON result is written to stdout. PDF page numbers are physical pages.
For spreadsheets and other office documents, labels identify sheets/sections.
This program never modifies the source document or executes embedded content.
"""

import argparse
import csv
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import resource
import signal
import subprocess
import sys
import tempfile
import time
import unicodedata
import xml.etree.ElementTree as ET
import zipfile


MAX_INPUT_BYTES = 10 * 1024 * 1024
MAX_UNPACKED_BYTES = 64 * 1024 * 1024
MAX_ENTRIES = 5000
MAX_NODES = 250000
MAX_DEPTH = 100
MAX_ROWS = 100000
MAX_COLUMNS = 1024
SUPPORTED = {'.pdf', '.docx', '.odt', '.xlsx', '.ods', '.csv', '.txt', '.json', '.xml'}


class ExtractionError(Exception):
    pass


def clean(value):
    value = unicodedata.normalize('NFC', value.replace('\r\n', '\n').replace('\r', '\n'))
    value = ''.join(c for c in value if c in '\n\t' or unicodedata.category(c) != 'Cc')
    return re.sub(r'\n[ \t]*\n(?:[ \t]*\n)+', '\n\n', value).strip()


class Output:
    def __init__(self, args):
        self.args = args
        self.pages = []
        self.size = 0
        self.parts = []
        self.part_size = 0
        self.warnings = []
        self.methods = set()

    def line(self, text):
        text = clean(str(text))
        if text:
            self.part_size += len(text) + 1
            if self.size + self.part_size > self.args.max_output_chars:
                raise ExtractionError('Text překračuje povolenou velikost. Dokument nebyl zaindexován.')
            self.parts.append(text)

    def page(self, number, label, method='text'):
        text = '\n'.join(self.parts)
        self.parts = []
        self.part_size = 0
        self.size += len(text)
        if len(self.pages) >= self.args.max_pages:
            raise ExtractionError('Dokument překračuje povolený počet stran nebo listů.')
        self.pages.append({'page': number, 'label': clean(label)[:200], 'text': text})
        self.methods.add(method)

    def result(self):
        if not any(p['text'].strip() for p in self.pages):
            raise ExtractionError('V dokumentu nebyl nalezen čitelný text.')
        return {
            'status': 'ready',
            'pages': self.pages,
            'method': next(iter(self.methods)) if len(self.methods) == 1 else 'mixed',
            'warnings': self.warnings,
        }


def xml(data):
    # Check before parsing, including UTF-16/32 documents. External entities and
    # internal entity declarations have no purpose in supported attachment text.
    if re.search(br'<!\s*(?:DOCTYPE|ENTITY)\b', data.replace(b'\x00', b''), re.I):
        raise ExtractionError('XML s deklaracemi DTD nebo entit není podporováno.')
    if len(data) > MAX_UNPACKED_BYTES:
        raise ExtractionError('Rozbalený dokument je příliš velký.')
    parser = ET.XMLPullParser(events=('start', 'end'))
    depth = count = 0
    root = None
    try:
        for offset in range(0, len(data), 65536):
            parser.feed(data[offset:offset + 65536])
            for event, element in parser.read_events():
                if event == 'start':
                    if root is None:
                        root = element
                    depth += 1
                    count += 1
                    if depth > MAX_DEPTH or count > MAX_NODES:
                        raise ExtractionError('XML překračuje limit hloubky nebo počtu položek.')
                else:
                    depth -= 1
        parser.close()
    except ET.ParseError as exc:
        raise ExtractionError('Neplatný XML dokument.') from exc
    if root is None:
        raise ExtractionError('Prázdný XML dokument.')
    return root


def local(tag):
    return tag.rsplit('}', 1)[-1]


class OfficeZip:
    def __init__(self, path):
        try:
            self.archive = zipfile.ZipFile(path)
            entries = self.archive.infolist()
        except (zipfile.BadZipFile, OSError) as exc:
            raise ExtractionError('Neplatný kancelářský dokument.') from exc
        names = [entry.filename for entry in entries]
        if len(entries) > MAX_ENTRIES or len(set(names)) != len(names):
            raise ExtractionError('Archiv má příliš mnoho položek nebo duplicitní názvy.')
        if sum(entry.file_size for entry in entries) > MAX_UNPACKED_BYTES:
            raise ExtractionError('Rozbalený dokument překračuje limit 64 MiB.')
        for entry in entries:
            if entry.flag_bits & 1:
                raise ExtractionError('Zašifrovaný dokument není podporován.')
            name = PurePosixPath(entry.filename)
            if name.is_absolute() or '..' in name.parts or len(entry.filename) > 1024:
                raise ExtractionError('Neplatná cesta v kancelářském dokumentu.')
        self.names = set(names)

    def read(self, name):
        if name not in self.names:
            raise ExtractionError('V kancelářském dokumentu chybí povinná část.')
        try:
            with self.archive.open(name) as source:
                data = source.read(MAX_UNPACKED_BYTES + 1)
        except (zipfile.BadZipFile, RuntimeError, OSError) as exc:
            raise ExtractionError('Kancelářský dokument nelze rozbalit.') from exc
        if len(data) > MAX_UNPACKED_BYTES:
            raise ExtractionError('Rozbalená část dokumentu je příliš velká.')
        return data

    def xml(self, name):
        return xml(self.read(name))


W = '{http://schemas.openxmlformats.org/wordprocessingml/2006/main}'
S = '{http://schemas.openxmlformats.org/spreadsheetml/2006/main}'
REL = '{http://schemas.openxmlformats.org/officeDocument/2006/relationships}'
T = '{urn:oasis:names:tc:opendocument:xmlns:text:1.0}'
TAB = '{urn:oasis:names:tc:opendocument:xmlns:table:1.0}'
OFF = '{urn:oasis:names:tc:opendocument:xmlns:office:1.0}'


def word_text(element):
    parts = []
    def visit(node):
        if node.tag == W + 't' and node.text:
            parts.append(node.text)
        elif node.tag == W + 'tab':
            parts.append('\t')
        elif node.tag in (W + 'br', W + 'cr'):
            parts.append('\n')
        else:
            for child in node:
                visit(child)
            if node.tag == W + 'p':
                parts.append('\n')
    visit(element)
    return ''.join(parts).strip()


def table_rows(rows, out):
    headers = None
    for index, row in enumerate(rows):
        if index >= MAX_ROWS or len(row) > MAX_COLUMNS:
            raise ExtractionError('Tabulka překračuje povolený počet řádků nebo sloupců.')
        row = [clean(str(v)) for v in row]
        if not any(row):
            continue
        if headers is None:
            headers = row
            out.line(' | '.join(row))
            continue
        out.line(' | '.join(
            f'{headers[col]}: {value}' if col < len(headers) and headers[col] else value
            for col, value in enumerate(row) if value
        ))


def docx(path, out):
    archive = OfficeZip(path)
    root = archive.xml('word/document.xml')
    if root.tag != W + 'document':
        raise ExtractionError('Soubor není platný DOCX dokument.')
    body = root.find(W + 'body')
    if body is None:
        raise ExtractionError('DOCX nemá tělo dokumentu.')
    def visit(node):
        for child in node:
            if child.tag == W + 'p':
                out.line(word_text(child))
            elif child.tag == W + 'tbl':
                table_rows(([word_text(cell) for cell in row.findall(W + 'tc')]
                            for row in child.findall(W + 'tr')), out)
            else:
                visit(child)
    visit(body)
    out.page(1, 'Dokument')
    extras = sorted(name for name in archive.names if re.fullmatch(
        r'word/(?:header\d+|footer\d+|footnotes|endnotes)\.xml', name))
    for name in extras:
        root = archive.xml(name)
        for paragraph in root.iter(W + 'p'):
            out.line(word_text(paragraph))
        if out.parts:
            out.page(len(out.pages) + 1, 'Poznámky a záhlaví')


def odf_text(node):
    parts = [node.text or '']
    for child in node:
        if child.tag == T + 's':
            repeat = positive_int(child.get(T + 'c', '1'), 10000)
            parts.append(' ' * repeat)
        elif child.tag == T + 'tab':
            parts.append('\t')
        elif child.tag == T + 'line-break':
            parts.append('\n')
        else:
            parts.append(odf_text(child))
        parts.append(child.tail or '')
    return ''.join(parts)


def positive_int(value, maximum):
    try:
        result = int(value)
    except (ValueError, TypeError) as exc:
        raise ExtractionError('Neplatný počet položek v dokumentu.') from exc
    if result < 1 or result > maximum:
        raise ExtractionError('Dokument překračuje povolený počet opakovaných položek.')
    return result


def odf_rows(table):
    produced = 0
    for row in table.iter(TAB + 'table-row'):
        cells = []
        for cell in row:
            if cell.tag not in (TAB + 'table-cell', TAB + 'covered-table-cell'):
                continue
            value = '\n'.join(odf_text(p) for p in cell.iter() if p.tag in (T + 'p', T + 'h'))
            if not value:
                value = (cell.get(OFF + 'string-value') or cell.get(OFF + 'date-value')
                         or cell.get(OFF + 'value') or cell.get(OFF + 'boolean-value') or '')
            repeat_raw = cell.get(TAB + 'number-columns-repeated', '1')
            # Office writers often represent all trailing empty columns at once.
            if not value and int(repeat_raw) > MAX_COLUMNS:
                continue
            repeat = positive_int(repeat_raw, MAX_COLUMNS)
            if len(cells) + repeat > MAX_COLUMNS:
                raise ExtractionError('Tabulka má příliš mnoho sloupců.')
            cells.extend([value] * repeat)
        if not any(cells):
            continue
        repeat = positive_int(row.get(TAB + 'number-rows-repeated', '1'), MAX_ROWS)
        produced += repeat
        if produced > MAX_ROWS:
            raise ExtractionError('Tabulka má příliš mnoho řádků.')
        for _ in range(repeat):
            yield cells


def odf(path, out, spreadsheet):
    archive = OfficeZip(path)
    expected = b'application/vnd.oasis.opendocument.' + (b'spreadsheet' if spreadsheet else b'text')
    if archive.read('mimetype').strip() != expected:
        raise ExtractionError('Typ OpenDocument neodpovídá příponě souboru.')
    root = archive.xml('content.xml')
    body = root.find(OFF + 'body')
    content = body.find(OFF + ('spreadsheet' if spreadsheet else 'text')) if body is not None else None
    if content is None:
        raise ExtractionError('OpenDocument nemá požadovaný obsah.')
    if spreadsheet:
        for table in content.findall(TAB + 'table'):
            table_rows(odf_rows(table), out)
            out.page(len(out.pages) + 1, 'List: ' + table.get(TAB + 'name', 'Tabulka'))
    else:
        def visit(node):
            for child in node:
                if child.tag in (T + 'p', T + 'h'):
                    out.line(odf_text(child))
                elif child.tag == TAB + 'table':
                    table_rows(odf_rows(child), out)
                else:
                    visit(child)
        visit(content)
        out.page(1, 'Dokument')


def xlsx(path, out):
    archive = OfficeZip(path)
    root = archive.xml('xl/workbook.xml')
    if root.tag != S + 'workbook':
        raise ExtractionError('Soubor není platný XLSX dokument.')
    relations = {}
    for relation in archive.xml('xl/_rels/workbook.xml.rels'):
        if relation.get('TargetMode') == 'External':
            continue
        target = relation.get('Target', '')
        if target.startswith('/'):
            target = target.lstrip('/')
        else:
            target = 'xl/' + target
        if '..' in PurePosixPath(target).parts:
            raise ExtractionError('Neplatný odkaz na list tabulky.')
        relations[relation.get('Id')] = target
    shared = []
    if 'xl/sharedStrings.xml' in archive.names:
        for string in archive.xml('xl/sharedStrings.xml'):
            shared.append(''.join(node.text or '' for node in string.iter(S + 't')))
    sheets = root.find(S + 'sheets')
    if sheets is None:
        raise ExtractionError('Tabulka neobsahuje listy.')
    for sheet in sheets:
        target = relations.get(sheet.get(REL + 'id'))
        if not target:
            raise ExtractionError('Tabulka odkazuje na neplatný nebo externí list.')
        sheet_root = archive.xml(target)
        def rows():
            for row in sheet_root.iter(S + 'row'):
                values = []
                for cell in row.findall(S + 'c'):
                    ref = cell.get('r', '')
                    match = re.fullmatch(r'([A-Z]+)[1-9][0-9]*', ref)
                    column = len(values)
                    if match:
                        column = 0
                        for letter in match.group(1):
                            column = column * 26 + ord(letter) - ord('A') + 1
                        column -= 1
                    if column >= MAX_COLUMNS:
                        raise ExtractionError('Tabulka má příliš mnoho sloupců.')
                    value_node = cell.find(S + 'v')
                    value = value_node.text or '' if value_node is not None else ''
                    kind = cell.get('t')
                    if kind == 's':
                        try:
                            index = int(value)
                            if index < 0:
                                raise IndexError()
                            value = shared[index]
                        except (ValueError, IndexError) as exc:
                            raise ExtractionError('Neplatný odkaz na text tabulky.') from exc
                    elif kind == 'inlineStr':
                        value = ''.join(node.text or '' for node in cell.iter(S + 't'))
                    elif kind == 'b':
                        value = 'ano' if value == '1' else 'ne'
                    while len(values) <= column:
                        values.append('')
                    values[column] = value
                yield values
        table_rows(rows(), out)
        out.page(len(out.pages) + 1, 'List: ' + sheet.get('name', 'Tabulka'))


def utf8(data):
    try:
        return data.decode('utf-8-sig')
    except UnicodeDecodeError as exc:
        raise ExtractionError('Textový soubor musí být uložen v UTF-8.') from exc


def structured_json(path, out):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ExtractionError('JSON obsahuje duplicitní názvy položek.')
            result[key] = value
        return result
    def invalid_constant(value):
        raise ExtractionError('JSON obsahuje neplatnou číselnou hodnotu.')
    try:
        root = json.loads(utf8(path.read_bytes()), object_pairs_hook=unique_object, parse_constant=invalid_constant)
    except (json.JSONDecodeError, RecursionError) as exc:
        raise ExtractionError('Neplatný nebo příliš hluboký JSON.') from exc
    count = 0
    def visit(value, key, depth):
        nonlocal count
        count += 1
        if depth > MAX_DEPTH or count > MAX_NODES:
            raise ExtractionError('JSON překračuje limit hloubky nebo počtu položek.')
        if isinstance(value, dict):
            for name, child in value.items():
                visit(child, f'{key}.{name}' if key else name, depth + 1)
        elif isinstance(value, list):
            for index, child in enumerate(value):
                visit(child, f'{key}[{index + 1}]', depth + 1)
        else:
            text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
            out.line(f'{key}: {text}' if key else text)
    visit(root, '', 0)
    out.page(1, 'Data JSON')


def structured_xml(path, out):
    root = xml(path.read_bytes())
    def visit(node, prefix):
        key = f'{prefix}/{local(node.tag)}' if prefix else local(node.tag)
        for name, value in node.attrib.items():
            out.line(f'{key}/@{local(name)}: {value}')
        if node.text and node.text.strip():
            out.line(f'{key}: {node.text}')
        for child in node:
            visit(child, key)
            if child.tail and child.tail.strip():
                out.line(f'{key}: {child.tail}')
    visit(root, '')
    out.page(1, 'Data XML')


def command(args, directory, output=None):
    """Use capped regular files for external output, never unbounded PIPE buffers."""
    error_path = Path(directory) / 'command-error.txt'
    stdout_path = output or Path(directory) / 'command-output.txt'
    env = dict(os.environ, OMP_THREAD_LIMIT='1', OMP_NUM_THREADS='1', OPENBLAS_NUM_THREADS='1', LC_ALL='C')
    try:
        with open(stdout_path, 'wb') as stdout, open(error_path, 'wb') as stderr:
            process = subprocess.run(args, stdout=stdout, stderr=stderr, env=env, check=False)
    except FileNotFoundError as exc:
        raise ExtractionError(f'Chybí nástroj {args[0]}.') from exc
    if process.returncode:
        # Avoid exposing document text or system paths in persisted errors.
        raise ExtractionError(f'Nástroj {args[0]} nedokončil zpracování (kód {process.returncode}).')
    if output is None:
        return stdout_path.read_text(encoding='utf-8', errors='replace')
    return ''


def pdf(path, out):
    if not path.read_bytes()[:1024].lstrip().startswith(b'%PDF-'):
        raise ExtractionError('Soubor není platné PDF.')
    with tempfile.TemporaryDirectory(prefix='attachment-pdf-') as directory:
        info = command(['pdfinfo', str(path)], directory)
        count_match = re.search(r'^Pages:\s+(\d+)\s*$', info, re.M)
        if not count_match:
            raise ExtractionError('Počet stran PDF nelze zjistit.')
        count = positive_int(count_match.group(1), out.args.max_pages)
        image_pages = set()
        images = command(['pdfimages', '-list', str(path)], directory)
        for line in images.splitlines():
            columns = line.split()
            if len(columns) >= 5 and columns[0].isdigit() and columns[2] == 'image':
                width, height = int(columns[3]), int(columns[4])
                if min(width, height) >= 250 and width * height >= 250000:
                    image_pages.add(int(columns[0]))
        for page in range(1, count + 1):
            target = Path(directory) / 'text.txt'
            command(['pdftotext', '-f', str(page), '-l', str(page), '-enc', 'UTF-8', '-layout', str(path), str(target)], directory)
            text = target.read_text(encoding='utf-8', errors='replace')
            method = 'text'
            meaningful = sum(c.isalnum() for c in text)
            damaged = text.count('\ufffd') > max(3, meaningful // 20)
            if meaningful < 25 or damaged or page in image_pages:
                prefix = Path(directory) / 'page'
                image = prefix.with_suffix('.png')
                command(['pdftoppm', '-f', str(page), '-l', str(page), '-singlefile', '-scale-to', '2400', '-gray', '-png', str(path), str(prefix)], directory)
                command(['tesseract', str(image), 'stdout', '-l', 'ces+eng', '--psm', '3', '--dpi', '200'], directory, output=target)
                recognized = target.read_text(encoding='utf-8', errors='replace')
                image.unlink(missing_ok=True)
                if meaningful >= 25 and not damaged:
                    # Preserve accurate native text and add image-only text. The
                    # rendered-page OCR may repeat native lines, so omit repeats.
                    key = lambda value: re.sub(r'\s+', ' ', clean(value)).casefold()
                    seen = {key(line) for line in text.splitlines()}
                    extra = []
                    for line in recognized.splitlines():
                        normalized = key(line)
                        if normalized and normalized not in seen:
                            extra.append(line)
                            seen.add(normalized)
                    text += '\n' + '\n'.join(extra)
                    method = 'mixed'
                else:
                    text = recognized
                    method = 'ocr'
            out.line(text)
            out.page(page, f'Strana {page}', method)
        if out.methods & {'ocr', 'mixed'}:
            out.warnings.append('Část textu vznikla rozpoznáváním obrazu a může obsahovat chyby.')


def extract(args):
    path = Path(args.input)
    if not path.is_file() or path.stat().st_size > MAX_INPUT_BYTES:
        raise ExtractionError('Soubor chybí nebo překračuje limit 10 MiB.')
    suffix = Path(args.name).suffix.lower()
    if suffix not in SUPPORTED:
        raise ExtractionError('Nepodporovaný formát dokumentu.')
    out = Output(args)
    if suffix == '.pdf':
        pdf(path, out)
    elif suffix == '.docx':
        docx(path, out)
    elif suffix in ('.odt', '.ods'):
        odf(path, out, suffix == '.ods')
    elif suffix == '.xlsx':
        xlsx(path, out)
    elif suffix == '.json':
        structured_json(path, out)
    elif suffix == '.xml':
        structured_xml(path, out)
    elif suffix == '.csv':
        text = utf8(path.read_bytes())
        try:
            dialect = csv.Sniffer().sniff(text[:32768], delimiters=',;\t|')
        except csv.Error:
            dialect = csv.excel
        csv.field_size_limit(min(args.max_output_chars, 1024 * 1024))
        table_rows(csv.reader(io.StringIO(text), dialect), out)
        out.page(1, 'Tabulka CSV')
    else:
        out.line(utf8(path.read_bytes()))
        out.page(1, 'Text')
    return out.result()


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument('--input', required=True)
    result.add_argument('--name', required=True)
    result.add_argument('--max-memory-mb', type=int, default=384)
    result.add_argument('--timeout-seconds', type=int, default=3600)
    result.add_argument('--max-pages', type=int, default=200)
    result.add_argument('--max-output-chars', type=int, default=2000000)
    result.add_argument('--worker', action='store_true', help=argparse.SUPPRESS)
    return result


def failed(message):
    return {'status': 'failed', 'pages': [], 'method': 'text', 'warnings': [], 'error': message}


def worker(args):
    memory = args.max_memory_mb * 1024 * 1024
    resource.setrlimit(resource.RLIMIT_AS, (memory, memory))
    # Applies to render output and stderr files too. The source is never altered.
    size = max(16 * 1024 * 1024, args.max_output_chars * 8)
    resource.setrlimit(resource.RLIMIT_FSIZE, (size, size))
    resource.setrlimit(resource.RLIMIT_CPU, (args.timeout_seconds, args.timeout_seconds + 1))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    started = time.monotonic()
    try:
        result = extract(args)
    except MemoryError:
        result = failed('Zpracování překročilo paměťový limit.')
    except ExtractionError as exc:
        result = failed(str(exc))
    except (ValueError, OSError, csv.Error, RuntimeError, RecursionError) as exc:
        result = failed(f'Dokument nelze zpracovat ({type(exc).__name__}).')
    result['duration_seconds'] = round(time.monotonic() - started, 3)
    result['peak_process_rss_kib'] = max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                                         resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result['status'] == 'ready' else 1


def main():
    args = parser().parse_args()
    if not (64 <= args.max_memory_mb <= 4096 and 1 <= args.timeout_seconds <= 3600
            and 1 <= args.max_pages <= 10000 and 1 <= args.max_output_chars <= 10000000):
        print(json.dumps(failed('Neplatné limity zpracování.'), ensure_ascii=False))
        return 1
    if args.worker:
        return worker(args)
    child = None
    def stop(signum=None, frame=None):
        if child is not None:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        if signum is not None:
            raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    started = time.monotonic()
    try:
        # The supervisor owns the workspace so it also removes rendered pages
        # when the worker must be killed before its own finally blocks execute.
        with tempfile.TemporaryDirectory(prefix='attachment-job-') as workspace, tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
            child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], '--worker'],
                                     stdout=output, stderr=errors, start_new_session=True,
                                     env=dict(os.environ, TMPDIR=workspace))
            try:
                child.wait(timeout=args.timeout_seconds)
            except subprocess.TimeoutExpired:
                stop()
                child.wait()
                result = failed('Zpracování překročilo časový limit.')
            else:
                output.seek(0)
                data = output.read(args.max_output_chars * 8 + 1024 * 1024)
                try:
                    result = json.loads(data)
                    if result.get('status') not in ('ready', 'failed'):
                        raise ValueError()
                except (ValueError, UnicodeDecodeError):
                    result = failed('Zpracování selhalo nebo překročilo povolené prostředky.')
                if child.returncode and result.get('status') == 'ready':
                    result = failed('Zpracování nebylo dokončeno.')
    finally:
        stop()
    result['duration_seconds'] = round(time.monotonic() - started, 3)
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result['status'] == 'ready' else 1


if __name__ == '__main__':
    sys.exit(main())
