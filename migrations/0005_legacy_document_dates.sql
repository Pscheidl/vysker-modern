-- Keep the source's date separate from this application's publication timestamp.
ALTER TABLE documents ADD COLUMN source_published_on DATE;
UPDATE documents d SET source_published_on=(s.metadata->'dates'->>'published_on')::date
FROM legacy_sources s WHERE s.document_id=d.id
AND s.metadata->'dates'->>'published_on' IS NOT NULL;

-- An imported file can also be a notice listed in a thematic source page.
-- Preserve the original document, download URL and immutable import provenance.
CREATE TABLE legacy_notice_imports (
    source_key TEXT PRIMARY KEY REFERENCES legacy_sources(source_key),
    notice_id BIGINT NOT NULL UNIQUE REFERENCES notices(id),
    imported_at TEXT NOT NULL,
    preview_published BOOLEAN NOT NULL DEFAULT FALSE,
    metadata JSONB NOT NULL
);
CREATE TRIGGER legacy_notice_imports_no_update BEFORE UPDATE ON legacy_notice_imports FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER legacy_notice_imports_no_delete BEFORE DELETE ON legacy_notice_imports FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER legacy_notice_imports_no_truncate BEFORE TRUNCATE ON legacy_notice_imports FOR EACH STATEMENT EXECUTE FUNCTION reject_protected_change();
