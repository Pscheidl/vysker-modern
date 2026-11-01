"""End-to-end extractor tests, run with python3 -m unittest tests/test_attachment_extractor.py."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/extract_attachment.py'
FIXTURES = ROOT / 'tests/fixtures/search'


class ExtractorTests(unittest.TestCase):
    def extract(self, path, *extra, env=None, name=None):
        process = subprocess.run([sys.executable, str(SCRIPT), '--input', str(path), '--name', name or path.name, *extra],
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=140, env=env)
        result = json.loads(process.stdout)
        self.assertEqual(process.returncode, 0 if result['status'] == 'ready' else 1, process.stderr)
        return result

    def test_all_non_pdf_formats_preserve_text_and_structured_values(self):
        for suffix in ['txt', 'docx', 'odt', 'xlsx', 'ods', 'csv', 'json', 'xml']:
            with self.subTest(format=suffix):
                result = self.extract(FIXTURES / ('obec.' + suffix))
                self.assertEqual(result['status'], 'ready', result)
                text = '\n'.join(page['text'] for page in result['pages'])
                self.assertIn('hospod', text.lower())
                self.assertIn('15000', text)
                self.assertEqual(result['method'], 'text')
                if suffix in ['xlsx', 'ods', 'csv']:
                    self.assertIn('Částka: 15000', text)
                if suffix in ['xlsx', 'ods']:
                    self.assertEqual(result['pages'][0]['label'], 'List: Hospoda')

    @unittest.skipUnless(shutil.which('pdftotext') and shutil.which('pdfinfo'), 'Poppler is required')
    def test_pdf_text_keeps_physical_page(self):
        result = self.extract(FIXTURES / 'text.pdf')
        self.assertEqual(result['status'], 'ready', result)
        self.assertEqual(result['method'], 'text')
        self.assertEqual(result['pages'][0]['page'], 1)
        self.assertIn('pronájem obecní hospody', result['pages'][0]['text'])

    @unittest.skipUnless(shutil.which('tesseract') and shutil.which('pdftoppm'), 'Tesseract and Poppler are required')
    def test_scanned_and_mixed_pdf_under_384_mib_address_limit(self):
        for name, method, page_count in [('scan.pdf', 'ocr', 1), ('mixed.pdf', 'mixed', 2), ('hybrid.pdf', 'mixed', 1)]:
            with self.subTest(name=name):
                result = self.extract(FIXTURES / name, '--max-memory-mb', '384')
                self.assertEqual(result['status'], 'ready', result)
                self.assertEqual(result['method'], method)
                self.assertEqual(len(result['pages']), page_count)
                self.assertIn('hospody', result['pages'][-1]['text'])
                self.assertIn('15000', result['pages'][-1]['text'])
                if name == 'hybrid.pdf':
                    self.assertIn('Elektronický podpis', result['pages'][0]['text'])
                self.assertLess(result['peak_process_rss_kib'], 384 * 1024)

    def test_text_limit_is_failure_without_partial_search_results(self):
        result = self.extract(FIXTURES / 'obec.txt', '--max-output-chars', '15')
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['pages'], [])

    def test_pdf_page_limit_is_explicit_failure(self):
        if not shutil.which('pdfinfo'):
            self.skipTest('Poppler is required')
        result = self.extract(FIXTURES / 'mixed.pdf', '--max-pages', '1')
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['pages'], [])

    def test_xml_entity_and_utf16_doctype_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            for encoding in ['utf-8', 'utf-16']:
                path = Path(directory) / 'entity.xml'
                path.write_bytes('<?xml version="1.0"?><!DOCTYPE root [<!ENTITY x "test">]><root>&x;</root>'.encode(encoding))
                result = self.extract(path)
                self.assertEqual(result['status'], 'failed')
                self.assertIn('DTD', result['error'])

    def test_expansion_limit_and_disguised_office_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'bomb.docx'
            with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as out:
                out.writestr('word/document.xml', b' ' * (65 * 1024 * 1024))
            result = self.extract(path)
            self.assertEqual(result['status'], 'failed')
            self.assertIn('64 MiB', result['error'])
            result = self.extract(FIXTURES / 'obec.odt', name='false.docx')
            self.assertEqual(result['status'], 'failed')

    def test_deep_json_and_xml_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            for suffix, content in [('json', '[' * 120 + '1' + ']' * 120), ('xml', '<a>' * 120 + 'text' + '</a>' * 120)]:
                path = Path(directory) / ('deep.' + suffix)
                path.write_text(content)
                result = self.extract(path)
                self.assertEqual(result['status'], 'failed')

    def test_duplicate_json_keys_are_not_silently_discarded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'duplicate.json'
            path.write_text('{"hlasovani":"pronajem", "hlasovani":"rozpocet"}')
            result = self.extract(path)
            self.assertEqual(result['status'], 'failed')
            self.assertIn('duplicitní', result['error'])

    def test_wall_timeout_kills_tool_and_returns_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            tools = Path(directory)
            script = tools / 'pdfinfo'
            script.write_text('#!/bin/sh\nsleep 30\nprintf "Pages: 1\\n"\n')
            script.chmod(0o755)
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'])
            result = self.extract(FIXTURES / 'text.pdf', '--timeout-seconds', '1', env=env)
            self.assertEqual(result['status'], 'failed')
            self.assertIn('časový limit', result['error'])
            self.assertLess(result['duration_seconds'], 3)

    def test_external_tools_inherit_memory_limit(self):
        with tempfile.TemporaryDirectory() as directory:
            tools = Path(directory)
            script = tools / 'pdfinfo'
            script.write_text(f'#!{sys.executable}\nvalue = bytearray(128 * 1024 * 1024)\nprint("Pages: 1")\n')
            script.chmod(0o755)
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'])
            result = self.extract(FIXTURES / 'text.pdf', '--max-memory-mb', '64', env=env)
            self.assertEqual(result['status'], 'failed')
            self.assertEqual(result['pages'], [])
            self.assertIn('pdfinfo', result['error'])


if __name__ == '__main__':
    unittest.main()
