-- Czech substring search keeps the existing literal multi-word behavior while
-- avoiding repeated normalization of every content row on every request.
SELECT pg_advisory_xact_lock(hashtextextended(current_database() || ':vysker-search-extension',0));
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
CREATE FUNCTION document_fold(value TEXT) RETURNS TEXT LANGUAGE SQL IMMUTABLE PARALLEL SAFE AS $$
 SELECT lower(translate(value,'ÁČĎÉĚÍŇÓŘŠŤÚŮÝŽáčďéěíňóřšťúůýž','acdeeinorstuuyzacdeeinorstuuyz'))
$$;
ALTER TABLE documents ADD COLUMN search_text TEXT NOT NULL DEFAULT '';
CREATE FUNCTION refresh_document_search() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 NEW.search_text=document_fold(CASE WHEN NEW.metadata_restricted THEN coalesce(NEW.public_title,'Záznam úřední desky #'||NEW.id) ELSE NEW.title||' '||NEW.description||' '||coalesce(NEW.reference_number,'')||' '||coalesce(NEW.issuer,'') END||' '||coalesce((SELECT name FROM categories WHERE id=NEW.category_id),'Ostatní'));
 RETURN NEW;
END $$;
CREATE TRIGGER documents_search BEFORE INSERT OR UPDATE OF title,description,reference_number,issuer,category_id,metadata_restricted,public_title ON documents
 FOR EACH ROW EXECUTE FUNCTION refresh_document_search();
UPDATE documents d SET search_text=document_fold(CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Záznam úřední desky #'||d.id) ELSE d.title||' '||d.description||' '||coalesce(d.reference_number,'')||' '||coalesce(d.issuer,'') END||' '||coalesce((SELECT name FROM categories WHERE id=d.category_id),'Ostatní'));
CREATE FUNCTION refresh_document_category_search() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 UPDATE documents SET search_text=document_fold(CASE WHEN metadata_restricted THEN coalesce(public_title,'Záznam úřední desky #'||id) ELSE title||' '||description||' '||coalesce(reference_number,'')||' '||coalesce(issuer,'') END||' '||NEW.name) WHERE category_id=NEW.id;
 RETURN NEW;
END $$;
CREATE TRIGGER categories_document_search AFTER UPDATE OF name ON categories
 FOR EACH ROW WHEN (OLD.name IS DISTINCT FROM NEW.name) EXECUTE FUNCTION refresh_document_category_search();
CREATE INDEX documents_search_public ON documents USING gin(search_text public.gin_trgm_ops)
 WHERE public_access AND NOT access_blocked;
-- Historical evidence queries use these predicates without loading history into lists.
CREATE INDEX document_postings_removed ON document_postings(document_id)
 WHERE certainty='documented' AND (removed_at IS NOT NULL OR removed_on IS NOT NULL);
-- A detail resolves accepted revision provenance by local document ID, rather
-- than testing a correlated subquery against every captured source item.
CREATE INDEX document_revisions_accepted_document ON document_import_revisions(accepted_document_id)
 WHERE accepted_document_id IS NOT NULL;

-- Measured semi-joins for every page were more expensive than the old board.
-- These booleans are transactional projections, never substitutes for evidence.
ALTER TABLE documents ADD COLUMN has_current_posting BOOLEAN NOT NULL DEFAULT FALSE,
 ADD COLUMN has_removed_posting BOOLEAN NOT NULL DEFAULT FALSE;
UPDATE documents d SET
 has_current_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id AND p.certainty='documented' AND p.removed_at IS NULL AND p.removed_on IS NULL),
 has_removed_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id AND p.certainty='documented' AND (p.removed_at IS NOT NULL OR p.removed_on IS NOT NULL));
CREATE FUNCTION refresh_document_posting_projection() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 UPDATE documents d SET
 has_current_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id AND p.certainty='documented' AND p.removed_at IS NULL AND p.removed_on IS NULL),
 has_removed_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id AND p.certainty='documented' AND (p.removed_at IS NOT NULL OR p.removed_on IS NOT NULL))
 WHERE d.id=NEW.document_id;
 RETURN NEW;
END $$;
CREATE TRIGGER document_postings_projection AFTER INSERT OR UPDATE ON document_postings
 FOR EACH ROW EXECUTE FUNCTION refresh_document_posting_projection();
CREATE INDEX documents_current_board ON documents(published_on DESC NULLS LAST,id DESC)
 WHERE public_access AND NOT access_blocked AND board_status='published' AND has_current_posting;
CREATE INDEX documents_public_archive ON documents(published_on DESC NULLS LAST,id DESC)
 WHERE public_access AND NOT access_blocked AND board_status IN ('withdrawn','archived') AND has_removed_posting;
CREATE FUNCTION validate_document_posting_projection() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 NEW.has_current_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=NEW.id AND p.certainty='documented' AND p.removed_at IS NULL AND p.removed_on IS NULL);
 NEW.has_removed_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=NEW.id AND p.certainty='documented' AND (p.removed_at IS NOT NULL OR p.removed_on IS NOT NULL));
 RETURN NEW;
END $$;
CREATE TRIGGER documents_posting_projection BEFORE INSERT OR UPDATE OF has_current_posting,has_removed_posting ON documents
 FOR EACH ROW EXECUTE FUNCTION validate_document_posting_projection();
