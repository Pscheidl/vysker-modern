-- Provenance is separate from publication evidence and subscriber notifications.
ALTER TABLE events ADD COLUMN start_time_known BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE events ADD COLUMN end_time_known BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE events ADD COLUMN end_date_known BOOLEAN NOT NULL DEFAULT TRUE;
CREATE TABLE legacy_sources (
    source_key TEXT PRIMARY KEY,
    source_url TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    imported_at TEXT NOT NULL,
    metadata JSONB NOT NULL,
    page_id BIGINT REFERENCES pages(id),
    notice_id BIGINT REFERENCES notices(id),
    document_id BIGINT REFERENCES documents(id),
    attachment_id BIGINT REFERENCES attachments(id),
    -- A deleted calendar entry keeps its import identity to prevent accidental reimport.
    event_id BIGINT,
    destination TEXT NOT NULL CHECK (destination LIKE '/%' AND destination NOT LIKE '//%'),
    CHECK (num_nonnulls(page_id,notice_id,document_id,event_id)=1),
    CHECK (attachment_id IS NULL OR document_id IS NOT NULL OR notice_id IS NOT NULL)
);
CREATE TRIGGER legacy_sources_no_update BEFORE UPDATE ON legacy_sources FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER legacy_sources_no_delete BEFORE DELETE ON legacy_sources FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER legacy_sources_no_truncate BEFORE TRUNCATE ON legacy_sources FOR EACH STATEMENT EXECUTE FUNCTION reject_protected_change();
