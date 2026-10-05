#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "${1:-}" != "--inside" ]]; then
  if [[ -z "${TEST_DATABASE_URL:-}" ]]; then
    # A private temporary cluster, separate from the persistent development instance.
    pg_test_root="$(mktemp -d "${TMPDIR:-/tmp}/vysker-pg-test.XXXXXX")"
    cleanup() {
      pg_ctl -D "$pg_test_root/database" -m immediate -w stop >/dev/null 2>&1 || true
      rm -rf -- "$pg_test_root"
    }
    trap cleanup EXIT
    initdb -D "$pg_test_root/database" -U vysker --auth-local=trust --auth-host=reject --encoding=UTF8 --no-locale >/dev/null
    pg_ctl -D "$pg_test_root/database" -l "$pg_test_root/server.log" -o "-h '' -k '$pg_test_root' -c max_connections=150" -w start >/dev/null
    export TEST_DATABASE_URL="postgresql://vysker@localhost/postgres?host=$pg_test_root"
  fi
  python3 scripts/test-postgres.py bash scripts/test.sh --inside "$@"
  exit
fi
shift
cargo test --locked --no-default-features --features ssr --lib --bins --tests
python3 tests/test_database.py
python3 tests/test_monitor.py
python3 tests/test_legacy.py
python3 tests/test_legacy_capture.py
python3 tests/test_legacy_archive.py
python3 tests/test_legacy_replace.py
python3 tests/test_legacy_galleries.py
python3 tests/test_legacy_sync.py
if [[ "${1:-}" == "--smtp" ]]; then
  cargo test --locked --no-default-features --features ssr --test smtp --test recovery -- --ignored
fi
