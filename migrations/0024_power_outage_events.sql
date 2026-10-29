-- A stable external identity makes repeated synchronization idempotent.
ALTER TABLE events
    ADD COLUMN source_provider TEXT,
    ADD COLUMN source_id TEXT,
    ADD COLUMN source_url TEXT,
    ADD CONSTRAINT events_source_identity UNIQUE (source_provider, source_id),
    ADD CONSTRAINT events_source_complete CHECK (
        (source_provider IS NULL AND source_id IS NULL AND source_url IS NULL)
        OR (source_provider IS NOT NULL AND source_provider='cez_distribuce'
            AND source_id IS NOT NULL AND length(source_id)>0
            AND source_url IS NOT NULL AND length(source_url)>0)
    );
