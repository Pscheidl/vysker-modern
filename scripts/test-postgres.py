#!/usr/bin/env python3
"""Run a command against a fresh database and drop only that database afterwards."""
import os
import subprocess
import sys
from postgres import temporary_database

with temporary_database() as database:
    raise SystemExit(subprocess.call(sys.argv[1:], env={**os.environ, 'TEST_DATABASE_URL': database}))
