-- One current proposal per original identity, separate from live content.
-- Keeping the captured bytes here makes the queue independent of crawler caches.
CREATE TABLE legacy_sync_reviews (
    source_key TEXT PRIMARY KEY REFERENCES legacy_sources(source_key) ON DELETE CASCADE,
    fingerprint TEXT NOT NULL,
    proposal JSONB NOT NULL,
    source_data BYTEA NOT NULL,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    reviewed_at TEXT
);
