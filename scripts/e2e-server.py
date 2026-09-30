#!/usr/bin/env python3
"""Launch a disposable browser-test server, never the developer's database."""
import json
import os
from pathlib import Path
import signal
from postgres import connect, temporary_database
import subprocess
import tempfile
import time


def main():
    root=Path(__file__).resolve().parents[1]
    os.chdir(root)
    binary=Path(os.environ.get('E2E_WEB_BINARY','target/debug/obecni-web')).resolve(strict=True)
    admin=Path(os.environ.get('E2E_ADMIN_BINARY','target/debug/obec-admin')).resolve(strict=True)
    with temporary_database() as database:
        env={k:v for k,v in os.environ.items() if not k.startswith(('OBEC_','LEPTOS_'))}
        env.update(OBEC_ADRESA='127.0.0.1:3107',OBEC_DATABAZE=database,
                   OBEC_VEREJNA_URL='http://127.0.0.1:3107',OBEC_PRODUCTION='false',
                   OBEC_UKAZKOVA_DATA='false',OBEC_PRIVACY_CONFIG=str(root/'config/privacy.example.json'),
                   OBEC_SMTP_HOST='127.0.0.1',OBEC_SMTP_PORT='9',OBEC_SMTP_TLS='none',
                   OBEC_EMAIL_OD='Vyskeř <e2e@vysker.test>',LEPTOS_SITE_ROOT=str(root/'target/site'))
        subprocess.run([str(admin),'create','e2e-admin@vysker.test','--password-stdin'],input='Disposable-browser-test-password!\n',text=True,env=env,check=True)
        with connect(database) as conn:
            for n in range(1,26):
                conn.execute("INSERT INTO notices(title,description,published_on,status) VALUES (%s,%s,'2020-01-01','published')",(f'Zkušební vyhláška {n:02d}','Záznam pro test stránkování.'))
        manifest=root/'target/e2e-fixture.json'
        manifest.parent.mkdir(parents=True,exist_ok=True)
        manifest.write_text(json.dumps({'database':str(database)}))
        process=subprocess.Popen([str(binary)],env=env)
        def stop(*_):
            if process.poll() is None: process.terminate()
        signal.signal(signal.SIGTERM,stop)
        signal.signal(signal.SIGINT,stop)
        try:
            raise SystemExit(process.wait())
        finally:
            stop()
            process.wait(timeout=30)
            manifest.unlink(missing_ok=True)


if __name__=='__main__': main()
