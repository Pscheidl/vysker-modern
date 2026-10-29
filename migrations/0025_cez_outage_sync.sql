-- Persist the polling lease across processes and restarts. Network requests never
-- hold the application's write lock or an open database transaction.
CREATE TABLE cez_outage_sync (
    town_code TEXT PRIMARY KEY,
    last_attempt_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    next_attempt_at TIMESTAMPTZ NOT NULL,
    run_token TEXT,
    last_error TEXT
);

CREATE TABLE cez_outage_imports (
    event_id BIGINT PRIMARY KEY REFERENCES events(id) ON DELETE RESTRICT,
    town_code TEXT NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    missing_since TIMESTAMPTZ
);
CREATE INDEX cez_outage_imports_town ON cez_outage_imports(town_code);
