#!/usr/bin/env python3
"""Package locally built release artifacts into a narrow Docker build context."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='New output directory, outside the checkout')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    if output.is_relative_to(root):
        parser.error('Use an output directory outside the checkout')
    binaries = [root / 'target/release' / name for name in ('obecni-web', 'obec-admin')]
    for binary in binaries:
        if not binary.is_file():
            parser.error(f'Build the release binary first: {binary.name}')
        versions = subprocess.check_output(['readelf', '--version-info', binary], text=True)
        required = {tuple(map(int, v.split('.'))) for v in re.findall(r'GLIBC_([0-9.]+)', versions)}
        if required and max(required) > (2, 36):
            parser.error(f'{binary.name} needs newer glibc than the Debian bookworm runtime')
    if not (root / 'target/site/pkg/obecni-web.wasm').is_file():
        parser.error('Build the Leptos release frontend first')
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    for binary in binaries:
        shutil.copy2(binary, output / binary.name)
    shutil.copytree(root / 'target/site', output / 'site')
    shutil.copy2(root / 'Dockerfile.prebuilt', output / 'Dockerfile')
    shutil.copy2(root / 'Cargo.toml', output / 'Cargo.toml')
    (output / 'scripts').mkdir()
    for name in ('database', 'monitor', 'postgres', 'legacy', 'legacy_galleries',
                 'legacy_import', 'legacy_scope', 'legacy_sync', 'legacy_archive',
                 'legacy_reimport', 'legacy_replace', 'legacy_notifications'):
        shutil.copy2(root / f'scripts/{name}.py', output / f'scripts/{name}.py')
    (output / 'config').mkdir()
    for name in ('legacy-vysker-pages.json', 'legacy-vysker-notices.json'):
        shutil.copy2(root / 'config' / name, output / 'config' / name)
    files = {}
    for path in sorted(output.rglob('*')):
        if path.is_file():
            files[str(path.relative_to(output))] = hashlib.sha256(path.read_bytes()).hexdigest()
    digest = hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()
    manifest = {
        'bundle_sha256': digest,
        'image_tag': f'obecni-web:staging-{digest[:12]}',
        'source_base_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
        'source_worktree_dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=root, text=True)),
        'files': files,
    }
    (output / 'release-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(manifest['image_tag'])
    print(f'Release context: {output}')


if __name__ == '__main__':
    main()
