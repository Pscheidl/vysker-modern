"""Shared PostgreSQL connections and disposable test databases."""
from contextlib import contextmanager
import os
from pathlib import Path
import secrets
import subprocess
from urllib.parse import urlsplit, urlunsplit, quote
import psycopg
from psycopg import sql
from psycopg.conninfo import conninfo_to_dict, make_conninfo


def database_url(value=None):
    if value:
        return value
    if path := os.environ.get('OBEC_DATABAZE_FILE'):
        return Path(path).read_text().strip()
    return os.environ.get('OBEC_DATABAZE', 'postgresql://vysker:vysker@127.0.0.1:5432/vysker')


def connect(value=None, **kwargs):
    return psycopg.connect(database_url(value), connect_timeout=5, **kwargs)


def run_tool(tool, database, *arguments, **kwargs):
    # Keep passwords out of process arguments and error messages.
    options = conninfo_to_dict(database)
    env = os.environ.copy()
    if password := options.pop('password', None):
        env['PGPASSWORD'] = password
    result = subprocess.run([tool, '--dbname', make_conninfo(**options), *arguments],
                            env=env, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f'{tool} failed (exit {result.returncode})')
    return result


def maintenance_url(database):
    return make_conninfo(database, dbname='postgres')


def create_database(database):
    name = conninfo_to_dict(database)['dbname']
    if name in ('postgres', 'template0', 'template1'):
        raise ValueError('Choose a new application database name')
    with connect(maintenance_url(database), autocommit=True) as control:
        control.execute(sql.SQL('CREATE DATABASE {}').format(sql.Identifier(name)))


def drop_database(database):
    name = conninfo_to_dict(database)['dbname']
    with connect(maintenance_url(database), autocommit=True) as control:
        control.execute(sql.SQL('DROP DATABASE {} WITH (FORCE)').format(sql.Identifier(name)))


def renamed_database(database, name):
    parts = urlsplit(database)
    if parts.scheme not in ('postgres', 'postgresql'):
        raise ValueError('Expected a PostgreSQL URL')
    return urlunsplit(parts._replace(path='/' + quote(name)))


@contextmanager
def temporary_database(base=None):
    base = base or os.environ.get('TEST_DATABASE_URL')
    if not base:
        raise RuntimeError('Set TEST_DATABASE_URL or run scripts/test.sh')
    target = renamed_database(base, 'vysker_test_' + secrets.token_hex(12))
    create_database(target)
    try:
        yield target
    finally:
        drop_database(target)
