#!/usr/bin/env bash
# Only runs when the production PostgreSQL volume is first initialized.
set -euo pipefail
export APP_DB_PASSWORD="$(cat "$APP_DB_PASSWORD_FILE")"
psql --username "$POSTGRES_USER" --dbname postgres --set ON_ERROR_STOP=1 <<'SQL'
\getenv app_password APP_DB_PASSWORD
CREATE ROLE vysker LOGIN PASSWORD :'app_password' NOSUPERUSER NOCREATEDB NOCREATEROLE;
CREATE DATABASE vysker OWNER vysker;
REVOKE ALL ON DATABASE vysker FROM PUBLIC;
GRANT CONNECT ON DATABASE vysker TO vysker;
SQL
unset APP_DB_PASSWORD
