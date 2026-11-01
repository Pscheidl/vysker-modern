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
    port=int(os.environ.get('E2E_PORT','3107'))
    if not 1024 <= port <= 65535:
        raise ValueError('E2E_PORT must be an unprivileged local port')
    base_url=f'http://127.0.0.1:{port}'
    with temporary_database() as database:
        env={k:v for k,v in os.environ.items() if not k.startswith(('OBEC_','LEPTOS_'))}
        env.update(OBEC_ADRESA=f'127.0.0.1:{port}',OBEC_DATABAZE=database,
                   OBEC_VEREJNA_URL=base_url,OBEC_PRODUCTION='false',
                   OBEC_UKAZKOVA_DATA='false',OBEC_PRIVACY_CONFIG=str(root/'config/privacy.example.json'),
                   OBEC_SMTP_HOST='127.0.0.1',OBEC_SMTP_PORT='9',OBEC_SMTP_TLS='none',
                   OBEC_EMAIL_OD='Vyskeř <e2e@vysker.test>',LEPTOS_SITE_ROOT=str(root/'target/site'))
        subprocess.run([str(admin),'create','e2e-admin@vysker.test','--password-stdin'],input='Disposable-browser-test-password!\n',text=True,env=env,check=True)
        with connect(database) as conn:
            # Synthetic fixture postings are explicit, with a chosen file policy.
            # Merely setting a legacy status must not manufacture posting history.
            for n in range(1,26):
                document_id = conn.execute("""
                    INSERT INTO documents(title,description,created_at,published_on,board_status,
                                          retain_attachments,posting_after_action)
                    VALUES (%s,%s,'2020-01-01T11:00:00Z','2020-01-01','draft',FALSE,'hide')
                    RETURNING id
                """, (f'Zkušební vyhláška {n:02d}','Záznam pro test stránkování.')).fetchone()[0]
                conn.execute("""
                    UPDATE documents SET lock_state='locked',status='published',public_access=TRUE,
                        board_status='published',published_at='2020-01-01T11:00:00Z',
                        board_published_at='2020-01-01T11:00:00Z' WHERE id=%s
                """, (document_id,))
                posting_id = conn.execute("""
                    INSERT INTO document_postings(document_id,origin,certainty,started_on,
                                                  started_at,after_action,snapshot_sha256)
                    VALUES (%s,'local','documented','2020-01-01','2020-01-01T11:00:00Z',
                            'hide',document_fingerprint(%s)) RETURNING id
                """, (document_id,document_id)).fetchone()[0]
                conn.execute("""
                    INSERT INTO document_events(document_id,kind,reason,payload)
                    VALUES (%s,'posted','Synthetic browser fixture',%s::jsonb)
                """, (document_id,json.dumps({'posting_id':posting_id,'after_action':'hide'})))
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
