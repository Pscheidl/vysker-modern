-- Permanent canonical identity prevents reannouncement across former models,
-- migration, reimport, repeated posting, outbox delivery and mail retention.
CREATE TABLE document_announcements (
 document_id BIGINT PRIMARY KEY REFERENCES documents(id),
 recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 origin TEXT NOT NULL CHECK(origin IN ('existing_content','historical_import','native'))
);
INSERT INTO document_announcements(document_id,origin)
 SELECT id,'existing_content' FROM documents d WHERE d.public_access OR d.published_at IS NOT NULL
 OR EXISTS(SELECT 1 FROM legacy_sources s WHERE s.document_id=d.id)
 OR EXISTS(SELECT 1 FROM publication_outbox o WHERE coalesce(o.notice_id,o.document_id)=d.id)
 OR EXISTS(SELECT 1 FROM mail_publication_items o WHERE coalesce(o.notice_id,o.document_id)=d.id);
CREATE TRIGGER document_announcements_immutable BEFORE UPDATE OR DELETE ON document_announcements FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_announcements_no_truncate BEFORE TRUNCATE ON document_announcements EXECUTE FUNCTION reject_protected_change();
CREATE FUNCTION claim_document_announcement(target BIGINT,at_time TIMESTAMPTZ) RETURNS BOOLEAN LANGUAGE plpgsql AS $$
DECLARE claimed BIGINT; BEGIN
 INSERT INTO document_announcements(document_id,recorded_at,origin) VALUES(target,at_time,'native') ON CONFLICT DO NOTHING RETURNING document_id INTO claimed;
 RETURN claimed IS NOT NULL;
END $$;
CREATE FUNCTION suppress_historical_document_mail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF NEW.document_id IS NOT NULL THEN
  INSERT INTO document_announcements(document_id,origin) VALUES(NEW.document_id,'historical_import') ON CONFLICT DO NOTHING;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER legacy_sources_no_historical_mail AFTER INSERT ON legacy_sources FOR EACH ROW EXECUTE FUNCTION suppress_historical_document_mail();
