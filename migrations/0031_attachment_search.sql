-- Derived, replaceable search data. Original attachments remain authoritative.
CREATE TABLE attachment_extractions (
    attachment_id BIGINT PRIMARY KEY REFERENCES attachments(id) ON DELETE CASCADE,
    source_sha256 TEXT NOT NULL CHECK (source_sha256 ~ '^[a-f0-9]{64}$'),
    source_name TEXT NOT NULL,
    extractor_version INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('processing','ready','failed')),
    method TEXT NOT NULL DEFAULT '',
    warnings JSONB NOT NULL DEFAULT '[]'::jsonb,
    error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

CREATE TABLE attachment_search_pages (
    attachment_id BIGINT NOT NULL REFERENCES attachment_extractions(attachment_id) ON DELETE CASCADE,
    page INTEGER NOT NULL CHECK (page > 0),
    label TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    search_text TEXT GENERATED ALWAYS AS (document_fold(content)) STORED,
    PRIMARY KEY (attachment_id,page)
);
CREATE INDEX attachment_search_pages_text ON attachment_search_pages USING GIN (search_text public.gin_trgm_ops);
CREATE INDEX attachment_extractions_retry ON attachment_extractions(next_attempt_at,attachment_id) WHERE status='failed';
CREATE INDEX attachments_public_filename_search ON attachments USING GIN (document_fold(name) public.gin_trgm_ops)
    WHERE data IS NOT NULL AND removed_at IS NULL AND is_public;

-- Invalidate inside the source mutation transaction, not on the worker's next poll.
-- This also erases derived content when retention removes the original bytes.
CREATE FUNCTION invalidate_attachment_extraction() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    IF NEW.data IS DISTINCT FROM OLD.data OR NEW.name IS DISTINCT FROM OLD.name
       OR NEW.sha256 IS DISTINCT FROM OLD.sha256 OR NEW.removed_at IS DISTINCT FROM OLD.removed_at
       OR NEW.is_public IS DISTINCT FROM OLD.is_public OR NEW.document_id IS DISTINCT FROM OLD.document_id THEN
        DELETE FROM attachment_extractions WHERE attachment_id=NEW.id;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER attachments_search_invalidation AFTER UPDATE OF data,name,sha256,removed_at,is_public,document_id ON attachments
    FOR EACH ROW EXECUTE FUNCTION invalidate_attachment_extraction();
