"""Exercise runtime imports using only the files included in release packages."""
import importlib.util
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
            (source / 'target/site/pkg/obecni-web.wasm').write_bytes(b'wasm fixture')
            context = temporary / 'package'

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
            self.assert_runtime_imports(context)

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
