#!/usr/bin/env python3
"""Package locally built release artifacts into a narrow Docker build context."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


def validate_frontend(root):
    """Require the cargo-leptos manifest and every referenced frontend artifact."""
    hash_file = root / 'target/release/hash.txt'
    if not hash_file.is_file():
        raise ValueError('Missing target/release/hash.txt. Build the Leptos release with hash-files enabled')
    hashes = {}
    for line in hash_file.read_text().splitlines():
        if not line.strip():
            continue
        kind, separator, digest = line.partition(':')
        kind, digest = kind.strip(), digest.strip()
        if not separator or not re.fullmatch(r'[A-Za-z0-9_-]+', digest) or kind in hashes:
            raise ValueError('Invalid cargo-leptos hash manifest')
        hashes[kind] = digest
    for kind in ('css', 'js', 'wasm'):
        if kind not in hashes:
            raise ValueError(f'Missing {kind} entry in cargo-leptos hash manifest')
        asset = root / 'target/site/pkg' / f'obecni-web.{hashes[kind]}.{kind}'
        if not asset.is_file() or asset.stat().st_size == 0:
            raise ValueError(f'Missing or empty hashed frontend artifact: {asset.name}')
    return hash_file


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
    try:
        hash_file = validate_frontend(root)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    for binary in binaries:
        shutil.copy2(binary, output / binary.name)
    shutil.copy2(hash_file, output / hash_file.name)
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
