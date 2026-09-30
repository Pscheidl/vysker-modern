#!/usr/bin/env bash
# Persistent local PostgreSQL supplied by nix develop.
set -euo pipefail
cd "$(dirname "$0")/.."
pg_data="$PWD/data/postgres"
pg_socket="$PWD/data/postgres-socket"
pg_port="${OBEC_PG_PORT:-5432}"
case "${1:-start}" in
  start)
    mkdir -p "$pg_data" "$pg_socket"
    chmod 700 "$pg_data" "$pg_socket"
    if [[ ! -f "$pg_data/PG_VERSION" ]]; then
      initdb -D "$pg_data" -U vysker --auth-local=trust --auth-host=scram-sha-256 \
        --pwfile=<(printf '%s\n' 'vysker') --encoding=UTF8 --no-locale >/dev/null
    fi
    if ! pg_ctl -D "$pg_data" status >/dev/null 2>&1; then
      pg_ctl -D "$pg_data" -l "$PWD/data/postgres.log" \
        -o "-h 127.0.0.1 -p $pg_port -k '$pg_socket'" -w start
    fi
    if [[ "$(PGPASSWORD=vysker psql -h 127.0.0.1 -p "$pg_port" -U vysker -d postgres -Atc "SELECT count(*) FROM pg_database WHERE datname='vysker'")" == 0 ]]; then
      PGPASSWORD=vysker createdb -h 127.0.0.1 -p "$pg_port" -U vysker vysker
    fi
    printf 'PostgreSQL ready on 127.0.0.1:%s, data in %s\n' "$pg_port" "$pg_data"
    ;;
  stop) pg_ctl -D "$pg_data" -m fast -w stop ;;
  status) pg_ctl -D "$pg_data" status ;;
  *) printf 'Usage: %s {start|stop|status}\n' "$0" >&2; exit 2 ;;
esac
