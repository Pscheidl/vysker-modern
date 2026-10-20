"""Exercise runtime imports using only the files included in release packages."""
import importlib.util
import io
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('package_prebuilt', ROOT / 'scripts/package-prebuilt.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class ReleasePackaging(unittest.TestCase):
    def create_source(self, temporary):
        source = temporary / 'source'
        source.mkdir()
        shutil.copytree(ROOT / 'scripts', source / 'scripts')
        shutil.copytree(ROOT / 'config', source / 'config')
        for name in ('Cargo.toml', 'Dockerfile.prebuilt'):
            shutil.copy2(ROOT / name, source / name)
        (source / 'target/release').mkdir(parents=True)
        for name in ('obecni-web', 'obec-admin'):
            (source / 'target/release' / name).write_bytes(b'release fixture')
        (source / 'target/site/pkg').mkdir(parents=True)
        hashes = {'css': 'css-fixture-hash', 'js': 'js_fixture_hash', 'wasm': 'wasm-fixture-hash'}
        (source / 'target/release/hash.txt').write_text(
            ''.join(f'{kind}: {digest}\n' for kind, digest in hashes.items()))
        for kind, digest in hashes.items():
            (source / 'target/site/pkg' / f'obecni-web.{digest}.{kind}').write_bytes(b'frontend fixture')
        return source

    def package_source(self, source, context):
        def build_metadata(command, **_):
            if command[0] == 'readelf':
                return 'GLIBC_2.36'
            if command[:3] == ['git', 'rev-parse', 'HEAD']:
                return '0' * 40 + '\n'
            if command[:3] == ['git', 'status', '--porcelain']:
                return ''
            raise AssertionError(command)

        # Only compiled-artifact metadata is synthetic. Packaging and all
        # Python runtime imports use the real release code.
        with patch.object(package, '__file__', str(source / 'scripts/package-prebuilt.py')), \
                patch.object(sys, 'argv', ['package-prebuilt.py', str(context)]), \
                patch.object(package.subprocess, 'check_output', side_effect=build_metadata), \
                patch('builtins.print'):
            package.main()

    def assert_runtime_imports(self, context):
        # Run outside the checkout so a missing packaged helper cannot be found
        # through the source directory. Do not connect to a database or run an import.
        for script, arguments in [('legacy_sync.py', ['run', '--help']),
                                  ('legacy_archive.py', ['--help'])]:
            result = subprocess.run(
                [sys.executable, str(context / 'scripts' / script), *arguments],
                cwd=context, capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('usage:', result.stdout)

    def test_prebuilt_bundle_can_start_legacy_commands(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            source = self.create_source(temporary)
            context = temporary / 'package'
            self.package_source(source, context)
            self.assertEqual((context / 'hash.txt').read_bytes(),
                             (source / 'target/release/hash.txt').read_bytes())
            manifest = json.loads((context / 'release-manifest.json').read_text())
            self.assertIn('hash.txt', manifest['files'])
            for asset in (source / 'target/site/pkg').iterdir():
                self.assertEqual((context / 'site/pkg' / asset.name).read_bytes(), asset.read_bytes())
                self.assertIn(f'site/pkg/{asset.name}', manifest['files'])
            self.assertIn('COPY obecni-web obec-admin hash.txt ./',
                          (context / 'Dockerfile').read_text())
            self.assert_runtime_imports(context)

    def test_prebuilt_bundle_rejects_missing_hash_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            source = self.create_source(temporary)
            (source / 'target/release/hash.txt').unlink()
            context = temporary / 'package'
            with patch('sys.stderr', new_callable=io.StringIO) as stderr, self.assertRaises(SystemExit):
                self.package_source(source, context)
            self.assertIn('Missing target/release/hash.txt', stderr.getvalue())
            self.assertFalse(context.exists())

    def test_prebuilt_bundle_rejects_missing_referenced_assets(self):
        for kind in ('css', 'js', 'wasm'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temporary:
                temporary = Path(temporary)
                source = self.create_source(temporary)
                next((source / 'target/site/pkg').glob(f'*.{kind}')).unlink()
                # A leftover file with the old, fixed name must not satisfy the check.
                (source / 'target/site/pkg' / f'obecni-web.{kind}').write_bytes(b'old frontend')
                context = temporary / 'package'
                with patch('sys.stderr', new_callable=io.StringIO) as stderr, self.assertRaises(SystemExit):
                    self.package_source(source, context)
                self.assertIn('Missing or empty hashed frontend artifact', stderr.getvalue())
                self.assertFalse(context.exists())

    def test_prebuilt_bundle_rejects_incomplete_or_invalid_hashes(self):
        for content in ('js: js_fixture_hash\nwasm: wasm-fixture-hash\n',
                        'css: ../outside\njs: js_fixture_hash\nwasm: wasm-fixture-hash\n',
                        'css: css-fixture-hash\ncss: duplicate\njs: js_fixture_hash\nwasm: wasm-fixture-hash\n'):
            with self.subTest(content=content), tempfile.TemporaryDirectory() as temporary:
                source = self.create_source(Path(temporary))
                (source / 'target/release/hash.txt').write_text(content)
                with self.assertRaises(ValueError):
                    package.validate_frontend(source)

    def test_docker_runtime_copy_can_start_legacy_commands(self):
        with tempfile.TemporaryDirectory() as temporary:
            context = Path(temporary)
            (context / 'scripts').mkdir()
            for line in (ROOT / 'Dockerfile.web').read_text().splitlines():
                if not line.startswith('COPY '):
                    continue
                arguments = shlex.split(line)[1:]
                for source in arguments[:-1]:
                    if source.startswith('scripts/') and source.endswith('.py'):
                        shutil.copy2(ROOT / source, context / 'scripts' / Path(source).name)
            self.assert_runtime_imports(context)


if __name__ == '__main__':
    unittest.main()
