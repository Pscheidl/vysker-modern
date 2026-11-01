-- Retention is separate from withdrawal and never provides an editorial unlock.
ALTER TABLE documents ADD COLUMN disposed_at TIMESTAMPTZ;
CREATE TABLE document_disposals (
 document_id BIGINT PRIMARY KEY REFERENCES documents(id),
 disposed_at TIMESTAMPTZ NOT NULL,
 policy_cutoff TIMESTAMPTZ NOT NULL,
 content_sha256 TEXT,
 attachment_hashes JSONB NOT NULL
);
CREATE TRIGGER document_disposals_immutable BEFORE UPDATE OR DELETE ON document_disposals FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_disposals_no_truncate BEFORE TRUNCATE ON document_disposals EXECUTE FUNCTION reject_protected_change();
-- Only a physically separate, guarded retention operation may erase expired bytes.
CREATE FUNCTION document_retention_authorized(target BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM document_disposals x JOIN documents d ON d.id=x.document_id
 JOIN privacy_maintenance p ON p.id=1 WHERE x.document_id=target AND NOT d.public_access
 AND d.board_status IN ('archived','withdrawn') AND d.withdrawn_at::timestamptz<x.policy_cutoff
 AND p.notice_delete_before::timestamptz=x.policy_cutoff)
$$;
-- The trigger permits only the exact tombstone. Lock and original fingerprints stay intact.
CREATE OR REPLACE FUNCTION protect_document() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE disposing BOOLEAN; BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'documents cannot be deleted by editorial operations'; END IF;
 IF TG_OP='UPDATE' THEN
  disposing=OLD.disposed_at IS NULL AND NEW.disposed_at IS NOT NULL AND document_retention_authorized(OLD.id)
   AND NEW.title='Skartovaný dokument #'||OLD.id AND NEW.description='' AND NEW.reference_number IS NULL AND NEW.issuer IS NULL;
  IF OLD.disposed_at IS NOT NULL AND (NEW.disposed_at IS DISTINCT FROM OLD.disposed_at OR NEW.public_access) THEN RAISE EXCEPTION 'disposed document cannot be restored by editing'; END IF;
  IF NEW.disposed_at IS DISTINCT FROM OLD.disposed_at AND NOT disposing THEN RAISE EXCEPTION 'retention is a separate process'; END IF;
  IF OLD.lock_state='draft' AND NEW.lock_state<>'draft' AND ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on) THEN
   RAISE EXCEPTION 'store final content before locking it' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
  IF OLD.lock_state<>'draft' AND (NEW.lock_state IS DISTINCT FROM OLD.lock_state OR NEW.locked_at IS DISTINCT FROM OLD.locked_at OR NEW.content_sha256 IS DISTINCT FROM OLD.content_sha256
   OR (NOT disposing AND ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on))) THEN
   RAISE EXCEPTION 'document content is permanently locked' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
  IF OLD.board_status='scheduled' AND ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on) THEN RAISE EXCEPTION 'scheduled content requires a new preview' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
  IF ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on) THEN NEW.content_version=OLD.content_version+1; END IF;
  IF NEW.category_id IS DISTINCT FROM OLD.category_id THEN
   INSERT INTO document_events(document_id,kind,payload) VALUES(OLD.id,'topic_changed',jsonb_build_object('old',OLD.category_id,'new',NEW.category_id)); END IF;
  IF NEW.access_blocked AND NEW.public_access THEN RAISE EXCEPTION 'locally blocked document cannot be public'; END IF;
  IF NEW.lock_state<>OLD.lock_state AND NEW.lock_state<>'draft' THEN NEW.locked_at=coalesce(NEW.locked_at,now()); NEW.content_sha256=document_fingerprint(OLD.id); END IF;
  IF NEW.status IS DISTINCT FROM OLD.status AND NEW.public_access IS NOT DISTINCT FROM OLD.public_access THEN NEW.public_access=(NEW.status='published') AND NOT NEW.access_blocked; END IF;
 ELSE NEW.public_access=NEW.public_access OR (NEW.status='published' AND NOT NEW.access_blocked);
 END IF;
 NEW.catalog_date=coalesce(NEW.document_date,NEW.source_published_on,CASE WHEN NOT EXISTS(SELECT 1 FROM legacy_sources s WHERE s.document_id=NEW.id) THEN NEW.published_at::date END);
 RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION protect_document_attachment() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE parent documents; previous documents; BEGIN
 NEW.document_id=coalesce(NEW.document_id,NEW.notice_id);
 SELECT * INTO parent FROM documents WHERE id=NEW.document_id FOR UPDATE;
 IF TG_OP='UPDATE' AND NEW.document_id=OLD.document_id AND NEW.data IS NULL AND NEW.removed_at IS NOT NULL
  AND NEW.name='Příloha #'||OLD.id AND NEW.sha256 IS NOT DISTINCT FROM OLD.sha256
  AND NEW.content_type=OLD.content_type AND NEW.size_bytes=OLD.size_bytes AND NEW.sort_order=OLD.sort_order
  AND NEW.is_public=OLD.is_public AND NEW.storage_path=OLD.storage_path AND NEW.notice_id IS NOT DISTINCT FROM OLD.notice_id
  AND document_retention_authorized(parent.id) THEN RETURN NEW; END IF;
 IF TG_OP='UPDATE' AND OLD.document_id<>NEW.document_id THEN
  SELECT * INTO previous FROM documents WHERE id=OLD.document_id FOR UPDATE;
  IF previous.lock_state<>'draft' THEN RAISE EXCEPTION 'locked attachment cannot move' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF; END IF;
 IF parent.lock_state<>'draft' OR parent.board_status='scheduled' THEN RAISE EXCEPTION 'document attachments are permanently locked' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
 IF NEW.data IS NOT NULL THEN NEW.sha256=encode(sha256(NEW.data),'hex'); NEW.size_bytes=octet_length(NEW.data); END IF;
 NEW.storage_path='attachment:'||NEW.id||':'||coalesce(NEW.sha256,'unavailable');
 UPDATE documents SET content_version=content_version+1 WHERE id=NEW.document_id;
 RETURN NEW;
END $$;
