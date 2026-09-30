#!/usr/bin/env python3
"""Start the full web with disposable data, then verify its production mode."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import socket
from postgres import connect, temporary_database
import subprocess
import tempfile
import time
import urllib.request


def start(binary, env):
    process = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    url = f"http://{env['OBEC_ADRESA']}"
    for _ in range(80):
        if process.poll() is not None:
            raise RuntimeError(process.stderr.read().decode())
        try:
            with urllib.request.urlopen(url + '/api/v1/ready', timeout=1) as response:
                if response.status == 200:
                    return process, url
        except (OSError, TimeoutError):
            time.sleep(0.25)
    process.terminate()
    process.wait(timeout=10)
    raise RuntimeError('Web failed to become ready')


def stop(process):
    process.terminate()
    process.wait(timeout=10)
    process.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/release/obecni-web')
    parser.add_argument('--site-root', default='target/production-site')
    parser.add_argument('--privacy-example', default='config/privacy.example.json')
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve(strict=True))
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='vysker-smoke-') as folder, temporary_database() as database:
        folder = Path(folder)
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(('OBEC_', 'LEPTOS_'))}
        env.update(OBEC_ADRESA=f'127.0.0.1:{port}', OBEC_DATABAZE=database,
                   OBEC_SMTP_TLS='none', OBEC_SMTP_HOST='127.0.0.1', OBEC_SMTP_PORT='9',
                   LEPTOS_SITE_ROOT=str(Path(args.site_root).resolve(strict=True)))
        process, url = start(binary, env)
        stop(process)
        # Synthetic approval exists only in this disposable test directory.
        policy = Path(args.privacy_example).read_text().replace('DOPLNIT', 'LOCAL TEST').replace('example.test', 'vysker.test')
        policy = json.loads(policy)
        policy['approved_on'] = dt.datetime.now(dt.timezone.utc).date().isoformat()
        privacy = folder / 'privacy.json'
        privacy.write_text(json.dumps(policy))
        env.update(OBEC_PRODUCTION='true', OBEC_VEREJNA_URL='https://production-smoke.vysker.test',
                   OBEC_PRIVACY_CONFIG=str(privacy), OBEC_SMTP_TLS='starttls')
        refused = subprocess.run([binary], env=env, capture_output=True, timeout=15)
        assert refused.returncode != 0 and b'kontakt' in refused.stderr, 'Missing content must block production startup'
        with connect(database) as connection:
            for slug in ('kontakt', 'obec', 'kalendar', 'pristupnost', 'povinne-informace'):
                connection.execute('INSERT INTO pages(slug,title,content,published,updated_at) VALUES (%s,%s,%s,TRUE,%s)',
                                   (slug, 'Test: ' + slug, 'Approved content fixture: ' + slug, '2026-09-29T00:00:00Z'))
        process, url = start(binary, env)
        try:
            for path in ('/', '/kontakt', '/pristupnost', '/povinne-informace', '/kalendar'):
                with urllib.request.urlopen(url + path, timeout=10) as response:
                    html = response.read().decode()
                    assert response.status == 200
                    assert 'Vývojový náhled' not in html
                    assert 'Podzimní setkání sousedů' not in html
                    assert 'strict-transport-security' in response.headers
                    if path != '/':
                        assert 'Approved content fixture: ' + path[1:] in html
            for file in ('obecni-web.js', 'obecni-web.wasm', 'obecni-web.css'):
                with urllib.request.urlopen(url + '/pkg/' + file, timeout=10) as response:
                    data = response.read()
                    assert response.status == 200 and len(data) > 100
                    if file.endswith('.wasm'):
                        assert data[:4] == b'\x00asm'
            print('Production smoke test: startup gate, real pages, no demo content, headers, JS/WASM/CSS OK')
        finally:
            stop(process)


if __name__ == '__main__':
    main()
