-- Maintenance-window migration. Stop every writer and take a verified pg_dump first.
-- There is one content table after this migration. `notices` is only a compatibility
-- projection over that table, never a second store or a UNION of content stores.
SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write',0));
LOCK TABLE notices,documents,attachments,legacy_sources IN ACCESS EXCLUSIVE MODE;

CREATE TABLE document_legacy_ids (
    old_model TEXT NOT NULL CHECK(old_model IN ('notice','document')),
    old_id BIGINT NOT NULL,
    document_id BIGINT NOT NULL REFERENCES documents(id),
    PRIMARY KEY(old_model,old_id)
);
ALTER TABLE documents
    ADD COLUMN reference_number TEXT,
    ADD COLUMN issuer TEXT,
    ADD COLUMN category_id BIGINT REFERENCES categories(id),
    ADD COLUMN document_date DATE,
    ADD COLUMN catalog_date DATE,
    ADD COLUMN updated_at TEXT NOT NULL DEFAULT to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
    ADD COLUMN public_access BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN access_blocked BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN public_until DATE,
    ADD COLUMN attachments_public BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN metadata_restricted BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN public_title TEXT,
    ADD COLUMN posting_after_action TEXT CHECK(posting_after_action IN ('retain','hide','anonymized')),
    ADD COLUMN posting_public_until DATE,
    ADD COLUMN posting_redacted_id BIGINT REFERENCES documents(id),
    ADD COLUMN scheduled_sha256 TEXT,
    ADD COLUMN lock_state TEXT NOT NULL DEFAULT 'draft' CHECK(lock_state IN ('draft','locked','review')),
    ADD COLUMN locked_at TIMESTAMPTZ,
    ADD COLUMN content_sha256 TEXT,
    ADD COLUMN content_version BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN board_status TEXT CHECK(board_status IN ('draft','scheduled','published','withdrawn','archived')),
    ADD COLUMN published_on DATE,
    ADD COLUMN withdraw_on DATE,
    ADD COLUMN withdrawn_at TEXT,
    ADD COLUMN board_published_at TEXT,
    ADD COLUMN retain_attachments BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN review_json TEXT NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(review_json::jsonb)='object');
UPDATE documents SET public_access=(status='published');
INSERT INTO document_legacy_ids SELECT 'document',id,id FROM documents;
-- Offset is above BOTH source sequences, including collisions and old notice URLs.
CREATE TEMP TABLE notice_id_map ON COMMIT DROP AS
    SELECT id AS old_id,id+(SELECT greatest(coalesce((SELECT max(id) FROM documents),0),coalesce((SELECT max(id) FROM notices),0))) AS new_id FROM notices;
INSERT INTO documents(id,title,description,status,created_at,published_at,source_published_on,reference_number,issuer,category_id,updated_at,public_access,public_until,board_status,published_on,withdraw_on,withdrawn_at,board_published_at,retain_attachments,review_json)
    SELECT m.new_id,n.title,coalesce(n.description,''),
      CASE WHEN n.status='published' OR (n.status IN ('archived','withdrawn') AND n.retain_attachments AND nullif(n.review_json::jsonb->>'archive_basis','') IS NOT NULL AND n.review_json::jsonb->>'archive_until' IS NOT NULL) THEN 'published' ELSE 'draft' END,
      n.created_at,n.published_at,n.published_on,n.reference_number,n.issuer,n.category_id,n.updated_at,
      n.status='published' OR (n.status IN ('archived','withdrawn') AND n.retain_attachments AND nullif(n.review_json::jsonb->>'archive_basis','') IS NOT NULL AND n.review_json::jsonb->>'archive_until' IS NOT NULL),
      CASE WHEN n.status IN ('archived','withdrawn') THEN (n.review_json::jsonb->>'archive_until')::date END,
      n.status,n.published_on,n.withdraw_on,n.withdrawn_at,n.published_at,n.retain_attachments,n.review_json
    FROM notices n JOIN notice_id_map m ON m.old_id=n.id;
-- Preserve the OLD effective public projection, including overdue worker rows.
-- Nonactive legacy board metadata was redacted independently of retained files.
UPDATE documents d SET
 public_access=(n.status IN ('published','withdrawn','archived') AND (n.status='archived' OR n.published_on IS NULL OR n.published_on<=(now() AT TIME ZONE 'Europe/Prague')::date)),
 metadata_restricted=NOT (n.status='published' AND (n.published_on IS NULL OR n.published_on<=(now() AT TIME ZONE 'Europe/Prague')::date) AND (n.withdraw_on IS NULL OR n.withdraw_on>(now() AT TIME ZONE 'Europe/Prague')::date)),
 public_title=coalesce(nullif(trim(n.review_json::jsonb->>'archive_title'),''),'Záznam úřední desky #'||n.id),
 attachments_public=(n.status='published' AND (n.published_on IS NULL OR n.published_on<=(now() AT TIME ZONE 'Europe/Prague')::date) AND (n.withdraw_on IS NULL OR n.withdraw_on>(now() AT TIME ZONE 'Europe/Prague')::date))
  OR coalesce((n.status IN ('published','withdrawn','archived') AND n.retain_attachments AND nullif(trim(n.review_json::jsonb->>'archive_basis'),'') IS NOT NULL AND (n.review_json::jsonb->>'archive_until')::date>(now() AT TIME ZONE 'Europe/Prague')::date),FALSE),
 public_until=NULL
 FROM notices n JOIN notice_id_map m ON m.old_id=n.id WHERE d.id=m.new_id;
UPDATE documents SET status=CASE WHEN public_access THEN 'published' ELSE 'draft' END WHERE board_status IS NOT NULL;
INSERT INTO document_legacy_ids SELECT 'notice',old_id,new_id FROM notice_id_map;
SELECT setval(pg_get_serial_sequence('documents','id'),greatest(coalesce((SELECT max(id) FROM documents),0),1),EXISTS(SELECT 1 FROM documents));

-- Repoint every FK first. Pending mail keeps its original topic and recipient snapshot.
DO $$ DECLARE r record; BEGIN
 FOR r IN SELECT conrelid::regclass AS tbl,conname FROM pg_constraint WHERE confrelid='notices'::regclass AND contype='f' LOOP
  EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I',r.tbl,r.conname);
 END LOOP;
END $$;
ALTER TABLE attachments DROP CONSTRAINT attachments_check;
DROP TRIGGER legacy_sources_no_update ON legacy_sources;
DROP TRIGGER legacy_notice_imports_no_update ON legacy_notice_imports;
DROP TRIGGER notice_events_no_update ON notice_events;
UPDATE attachments a SET notice_id=m.new_id,document_id=m.new_id FROM notice_id_map m WHERE a.notice_id=m.old_id;
UPDATE notice_events e SET notice_id=m.new_id FROM notice_id_map m WHERE e.notice_id=m.old_id;
UPDATE publication_outbox e SET notice_id=m.new_id FROM notice_id_map m WHERE e.notice_id=m.old_id;
UPDATE mail_publication_items e SET notice_id=m.new_id FROM notice_id_map m WHERE e.notice_id=m.old_id;
ALTER TABLE legacy_sources DROP CONSTRAINT legacy_sources_check;
UPDATE legacy_sources s SET notice_id=m.new_id,document_id=m.new_id,
    destination=CASE WHEN s.destination LIKE '/uredni-deska/%' THEN '/dokumenty/'||m.new_id ELSE s.destination END
    FROM notice_id_map m WHERE s.notice_id=m.old_id;
UPDATE legacy_notice_imports s SET notice_id=m.new_id FROM notice_id_map m WHERE s.notice_id=m.old_id;
ALTER TABLE attachments ADD CONSTRAINT attachments_canonical_parent CHECK(document_id IS NOT NULL AND (notice_id IS NULL OR notice_id=document_id));
ALTER TABLE attachments ADD CONSTRAINT attachments_notice_id_fkey FOREIGN KEY(notice_id) REFERENCES documents(id);
ALTER TABLE notice_events ADD CONSTRAINT notice_events_notice_id_fkey FOREIGN KEY(notice_id) REFERENCES documents(id);
ALTER TABLE legacy_sources ADD CONSTRAINT legacy_sources_notice_id_fkey FOREIGN KEY(notice_id) REFERENCES documents(id);
ALTER TABLE legacy_notice_imports ADD CONSTRAINT legacy_notice_imports_notice_id_fkey FOREIGN KEY(notice_id) REFERENCES documents(id);
ALTER TABLE publication_outbox ADD CONSTRAINT publication_outbox_notice_id_fkey FOREIGN KEY(notice_id) REFERENCES documents(id);
ALTER TABLE mail_publication_items ADD CONSTRAINT mail_publication_items_notice_id_fkey FOREIGN KEY(notice_id) REFERENCES documents(id);
ALTER TABLE legacy_sources ADD CONSTRAINT legacy_sources_canonical_target CHECK(num_nonnulls(page_id,document_id,event_id)=1 AND (notice_id IS NULL OR notice_id=document_id));
CREATE TRIGGER legacy_sources_no_update BEFORE UPDATE ON legacy_sources FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER legacy_notice_imports_no_update BEFORE UPDATE ON legacy_notice_imports FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER notice_events_no_update BEFORE UPDATE ON notice_events FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
DROP TABLE notices;

CREATE TABLE document_relations (
    id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    original_id BIGINT NOT NULL REFERENCES documents(id),
    related_id BIGINT NOT NULL REFERENCES documents(id),
    kind TEXT NOT NULL CHECK(kind IN ('correction','supplement','replacement','anonymized','source_change')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor_id BIGINT REFERENCES administrators(id),
    reason TEXT NOT NULL DEFAULT '',
    CHECK(original_id<>related_id), UNIQUE(original_id,related_id,kind)
);
CREATE INDEX document_relations_related ON document_relations(related_id);
CREATE TABLE document_events (
    id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    document_id BIGINT NOT NULL REFERENCES documents(id),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor_id BIGINT REFERENCES administrators(id),
    kind TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    payload JSONB NOT NULL DEFAULT '{}'
);
CREATE INDEX document_events_document ON document_events(document_id,id);
CREATE TABLE document_postings (
    id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    document_id BIGINT NOT NULL REFERENCES documents(id),
    origin TEXT NOT NULL CHECK(origin IN ('local','source','migration')),
    certainty TEXT NOT NULL CHECK(certainty IN ('documented','uncertain')),
    started_on DATE,
    started_at TIMESTAMPTZ,
    planned_removal_on DATE,
    removed_on DATE,
    removed_at TIMESTAMPTZ,
    after_action TEXT NOT NULL CHECK(after_action IN ('retain','hide','anonymized')),
    public_until DATE,
    redacted_document_id BIGINT REFERENCES documents(id),
    snapshot_sha256 TEXT,
    source_url TEXT,
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK(origin<>'local' OR started_at IS NOT NULL),
    CHECK(after_action<>'anonymized' OR redacted_document_id IS NOT NULL)
);
CREATE INDEX document_postings_document ON document_postings(document_id,id);
CREATE INDEX document_postings_current ON document_postings(document_id,planned_removal_on) WHERE removed_at IS NULL AND removed_on IS NULL AND certainty='documented';
CREATE UNIQUE INDEX document_postings_one_local_current ON document_postings(document_id) WHERE origin='local' AND removed_at IS NULL AND removed_on IS NULL;
UPDATE documents d SET catalog_date=coalesce(document_date,source_published_on,CASE WHEN NOT EXISTS(SELECT 1 FROM legacy_sources s WHERE s.document_id=d.id) THEN published_at::date END);
CREATE INDEX documents_catalog ON documents(catalog_date DESC NULLS LAST,id DESC) WHERE public_access AND NOT access_blocked;
CREATE INDEX documents_topic ON documents(category_id,source_published_on DESC NULLS LAST,id DESC) WHERE public_access AND NOT access_blocked;
CREATE INDEX documents_board ON documents(board_status,published_on DESC NULLS LAST,id DESC) WHERE public_access AND NOT access_blocked AND board_status IS NOT NULL;
CREATE INDEX documents_due ON documents(withdraw_on,id) WHERE board_status='published';
CREATE INDEX legacy_sources_document ON legacy_sources(document_id);
CREATE INDEX document_legacy_ids_document ON document_legacy_ids(document_id);

-- Files remain inside PostgreSQL. Immutable ID URLs go through access control.
ALTER TABLE attachments ADD COLUMN is_public BOOLEAN NOT NULL DEFAULT TRUE;
UPDATE attachments SET sha256=encode(sha256(data),'hex') WHERE data IS NOT NULL;
UPDATE attachments SET storage_path='attachment:'||id||':'||coalesce(sha256,'unavailable');
CREATE FUNCTION document_fingerprint(target BIGINT) RETURNS TEXT LANGUAGE SQL STABLE AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('title',d.title,'description',d.description,
  'reference_number',d.reference_number,'issuer',d.issuer,'document_date',d.document_date,
  'source_published_on',d.source_published_on,'attachments',coalesce((SELECT jsonb_agg(jsonb_build_object(
  'id',a.id,'name',a.name,'type',a.content_type,'size',a.size_bytes,'sha256',a.sha256,'order',a.sort_order,
  'removed',a.removed_at,'public',a.is_public) ORDER BY a.sort_order,a.id) FROM attachments a WHERE a.document_id=d.id),'[]'::jsonb))::text,'UTF8')),'hex') FROM documents d WHERE d.id=target
$$;
UPDATE documents SET posting_after_action=CASE WHEN retain_attachments THEN 'retain' ELSE 'hide' END,
 posting_public_until=(review_json::jsonb->>'archive_until')::date,
 scheduled_sha256=CASE WHEN board_status='scheduled' THEN document_fingerprint(id) END
 WHERE board_status IS NOT NULL;
CREATE FUNCTION document_is_public(d documents,day DATE) RETURNS BOOLEAN LANGUAGE SQL IMMUTABLE AS $$
 SELECT d.public_access AND NOT d.access_blocked AND (d.public_until IS NULL OR d.public_until>day)
$$;
-- Capture date/hash prove only the bytes available at migration, never historical fidelity.
INSERT INTO document_postings(document_id,origin,certainty,started_on,started_at,planned_removal_on,removed_on,removed_at,after_action,public_until,snapshot_sha256,source_url)
 SELECT d.id,CASE WHEN s.source_key IS NOT NULL THEN 'source' ELSE 'migration' END,
  CASE WHEN d.board_published_at IS NOT NULL OR (s.source_key IS NOT NULL AND d.published_on IS NOT NULL AND d.published_on<=coalesce((s.captured_at::timestamptz AT TIME ZONE 'Europe/Prague')::date,(now() AT TIME ZONE 'Europe/Prague')::date)) THEN 'documented' ELSE 'uncertain' END,
  d.published_on,d.board_published_at::timestamptz,d.withdraw_on,
  CASE WHEN d.withdrawn_at IS NOT NULL THEN (d.withdrawn_at::timestamptz AT TIME ZONE 'Europe/Prague')::date WHEN s.source_key IS NOT NULL AND coalesce(s.metadata->'source_dates'->>'removed_on',s.metadata->'dates'->>'removed_on')::date<=coalesce((s.captured_at::timestamptz AT TIME ZONE 'Europe/Prague')::date,(now() AT TIME ZONE 'Europe/Prague')::date) THEN coalesce(s.metadata->'source_dates'->>'removed_on',s.metadata->'dates'->>'removed_on')::date ELSE NULL END,
  d.withdrawn_at::timestamptz,CASE WHEN d.retain_attachments THEN 'retain' ELSE 'hide' END,
  (d.review_json::jsonb->>'archive_until')::date,document_fingerprint(d.id),s.source_url
 FROM documents d LEFT JOIN LATERAL(SELECT source_key,source_url,metadata,captured_at FROM legacy_sources WHERE document_id=d.id ORDER BY attachment_id NULLS FIRST,source_key LIMIT 1) s ON TRUE
 WHERE d.board_status IS NOT NULL AND (d.board_status IN ('published','archived','withdrawn') OR s.source_key IS NOT NULL);
UPDATE documents d SET lock_state=CASE WHEN EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id AND p.certainty='documented') THEN 'locked' ELSE 'review' END,
 locked_at=now(),content_sha256=document_fingerprint(d.id) WHERE EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id);
INSERT INTO document_events(document_id,kind,payload)
 SELECT id,'migration_captured',jsonb_build_object('sha256',content_sha256,'historical_bytes_verified',FALSE,'lock_state',lock_state) FROM documents WHERE lock_state<>'draft';

CREATE FUNCTION protect_document() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'documents cannot be deleted by editorial operations'; END IF;
 IF TG_OP='UPDATE' THEN
  IF OLD.lock_state='draft' AND NEW.lock_state<>'draft' AND ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on) THEN
   RAISE EXCEPTION 'store final content before locking it' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
  IF OLD.lock_state<>'draft' AND (NEW.lock_state IS DISTINCT FROM OLD.lock_state OR NEW.locked_at IS DISTINCT FROM OLD.locked_at OR NEW.content_sha256 IS DISTINCT FROM OLD.content_sha256
   OR ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on)) THEN
   RAISE EXCEPTION 'document content is permanently locked' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard';
  END IF;
  IF OLD.board_status='scheduled' AND ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on) THEN RAISE EXCEPTION 'scheduled content requires a new preview' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
  IF ROW(NEW.title,NEW.description,NEW.reference_number,NEW.issuer,NEW.document_date,NEW.source_published_on) IS DISTINCT FROM ROW(OLD.title,OLD.description,OLD.reference_number,OLD.issuer,OLD.document_date,OLD.source_published_on) THEN NEW.content_version=OLD.content_version+1; END IF;
  IF NEW.category_id IS DISTINCT FROM OLD.category_id THEN
   INSERT INTO document_events(document_id,kind,payload) VALUES(OLD.id,'topic_changed',jsonb_build_object('old',OLD.category_id,'new',NEW.category_id));
  END IF;
  IF NEW.access_blocked AND NEW.public_access THEN RAISE EXCEPTION 'locally blocked document cannot be public'; END IF;
  IF NEW.lock_state<>OLD.lock_state AND NEW.lock_state<>'draft' THEN
   NEW.locked_at=coalesce(NEW.locked_at,now()); NEW.content_sha256=document_fingerprint(OLD.id);
  END IF;
  -- Old general-document writers keep the public projection consistent.
  IF NEW.status IS DISTINCT FROM OLD.status AND NEW.public_access IS NOT DISTINCT FROM OLD.public_access THEN NEW.public_access=(NEW.status='published') AND NOT NEW.access_blocked; END IF;
 ELSE
  NEW.public_access=NEW.public_access OR (NEW.status='published' AND NOT NEW.access_blocked);
 END IF;
 NEW.catalog_date=coalesce(NEW.document_date,NEW.source_published_on,CASE WHEN NOT EXISTS(SELECT 1 FROM legacy_sources s WHERE s.document_id=NEW.id) THEN NEW.published_at::date END);
 RETURN NEW;
END $$;
CREATE TRIGGER documents_guard BEFORE INSERT OR UPDATE OR DELETE ON documents FOR EACH ROW EXECUTE FUNCTION protect_document();
CREATE TRIGGER documents_no_truncate BEFORE TRUNCATE ON documents EXECUTE FUNCTION reject_protected_change();
CREATE FUNCTION protect_document_attachment() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE parent documents; previous documents; BEGIN
 NEW.document_id=coalesce(NEW.document_id,NEW.notice_id);
 SELECT * INTO parent FROM documents WHERE id=NEW.document_id FOR UPDATE;
 IF TG_OP='UPDATE' AND OLD.document_id<>NEW.document_id THEN
  SELECT * INTO previous FROM documents WHERE id=OLD.document_id FOR UPDATE;
  IF previous.lock_state<>'draft' THEN RAISE EXCEPTION 'locked attachment cannot move' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
 END IF;
 IF parent.lock_state<>'draft' OR parent.board_status='scheduled' THEN RAISE EXCEPTION 'document attachments are permanently locked' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
 IF NEW.data IS NOT NULL THEN NEW.sha256=encode(sha256(NEW.data),'hex'); NEW.size_bytes=octet_length(NEW.data); END IF;
 NEW.storage_path='attachment:'||NEW.id||':'||coalesce(NEW.sha256,'unavailable');
 UPDATE documents SET content_version=content_version+1 WHERE id=NEW.document_id;
 RETURN NEW;
END $$;
CREATE TRIGGER attachments_document_guard BEFORE INSERT OR UPDATE ON attachments FOR EACH ROW EXECUTE FUNCTION protect_document_attachment();
CREATE FUNCTION normalize_legacy_document() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 NEW.document_id=coalesce(NEW.document_id,NEW.notice_id); RETURN NEW;
END $$;
CREATE TRIGGER legacy_sources_canonical BEFORE INSERT ON legacy_sources FOR EACH ROW EXECUTE FUNCTION normalize_legacy_document();
CREATE TRIGGER document_events_immutable BEFORE UPDATE OR DELETE ON document_events FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_events_no_truncate BEFORE TRUNCATE ON document_events EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_relations_immutable BEFORE UPDATE OR DELETE ON document_relations FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_postings_no_delete BEFORE DELETE ON document_postings FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE FUNCTION protect_document_posting() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF ROW(NEW.document_id,NEW.origin,NEW.certainty,NEW.started_on,NEW.started_at,NEW.snapshot_sha256,NEW.source_url,NEW.captured_at,NEW.after_action,NEW.public_until,NEW.redacted_document_id)
  IS DISTINCT FROM ROW(OLD.document_id,OLD.origin,OLD.certainty,OLD.started_on,OLD.started_at,OLD.snapshot_sha256,OLD.source_url,OLD.captured_at,OLD.after_action,OLD.public_until,OLD.redacted_document_id)
  OR (OLD.removed_at IS NOT NULL AND NEW.removed_at IS DISTINCT FROM OLD.removed_at)
  OR (OLD.removed_on IS NOT NULL AND NEW.removed_on IS DISTINCT FROM OLD.removed_on) THEN
  RAISE EXCEPTION 'actual posting evidence is append only' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER document_postings_guard BEFORE UPDATE ON document_postings FOR EACH ROW EXECUTE FUNCTION protect_document_posting();

CREATE VIEW notices AS SELECT id,title,reference_number,category_id,issuer,description,published_on,withdraw_on,withdrawn_at,retain_attachments,board_status AS status,created_at,updated_at,review_json,board_published_at AS published_at,lock_state,public_access,access_blocked,public_until,content_version,metadata_restricted,public_title,attachments_public FROM documents WHERE board_status IS NOT NULL;
CREATE FUNCTION write_notice_compatibility() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF TG_OP='INSERT' THEN
  INSERT INTO documents(title,description,reference_number,category_id,issuer,published_on,withdraw_on,withdrawn_at,retain_attachments,board_status,created_at,updated_at,review_json,board_published_at)
  VALUES(NEW.title,coalesce(NEW.description,''),NEW.reference_number,NEW.category_id,NEW.issuer,NEW.published_on,NEW.withdraw_on,NEW.withdrawn_at,coalesce(NEW.retain_attachments,FALSE),coalesce(NEW.status,'draft'),coalesce(NEW.created_at,to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"')),coalesce(NEW.updated_at,to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"')),coalesce(NEW.review_json,'{}'),NEW.published_at) RETURNING id INTO NEW.id;
 ELSE
  UPDATE documents SET title=NEW.title,description=coalesce(NEW.description,''),reference_number=NEW.reference_number,category_id=NEW.category_id,issuer=NEW.issuer,published_on=NEW.published_on,withdraw_on=NEW.withdraw_on,withdrawn_at=NEW.withdrawn_at,retain_attachments=NEW.retain_attachments,board_status=NEW.status,updated_at=NEW.updated_at,review_json=NEW.review_json,board_published_at=NEW.published_at WHERE id=OLD.id;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER notices_write INSTEAD OF INSERT OR UPDATE ON notices FOR EACH ROW EXECUTE FUNCTION write_notice_compatibility();

-- Posting creation also protects callers outside the web API against attachment races.
CREATE FUNCTION guard_posting_snapshot() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE parent documents; BEGIN
 SELECT * INTO parent FROM documents WHERE id=NEW.document_id FOR UPDATE;
 IF parent.lock_state='draft' THEN RAISE EXCEPTION 'lock complete document before posting' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
 IF NEW.snapshot_sha256 IS DISTINCT FROM document_fingerprint(NEW.document_id) THEN RAISE EXCEPTION 'posting snapshot differs from locked document' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER document_postings_snapshot BEFORE INSERT ON document_postings FOR EACH ROW EXECUTE FUNCTION guard_posting_snapshot();

CREATE TABLE document_migration_issues (
 id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
 document_id BIGINT NOT NULL REFERENCES documents(id),
 related_document_id BIGINT REFERENCES documents(id),
 kind TEXT NOT NULL,
 details JSONB NOT NULL
);
INSERT INTO document_migration_issues(document_id,kind,details)
 SELECT id,'uncertain_posting',jsonb_build_object('note','No inferred publication time. Content remains blocked for review.') FROM documents WHERE lock_state='review';
INSERT INTO document_migration_issues(document_id,related_document_id,kind,details)
 SELECT s.document_id,i.notice_id,'possible_legacy_duplicate',jsonb_build_object('source_key',s.source_key,'note','Historical reconciliation linked two authored records. Compare content and attachments before merging.')
 FROM legacy_notice_imports i JOIN legacy_sources s USING(source_key) WHERE s.document_id IS NOT NULL AND s.document_id<>i.notice_id;
CREATE TRIGGER document_migration_issues_immutable BEFORE UPDATE OR DELETE ON document_migration_issues FOR EACH ROW EXECUTE FUNCTION reject_protected_change();

CREATE FUNCTION document_files_public(d documents,day DATE) RETURNS BOOLEAN LANGUAGE SQL IMMUTABLE AS $$
 SELECT document_is_public(d,day) AND d.attachments_public AND
 (NOT (coalesce(d.board_status IN ('archived','withdrawn'),FALSE) OR d.metadata_restricted)
  OR d.posting_public_until IS NULL OR d.posting_public_until>day)
$$;

CREATE TRIGGER document_postings_no_truncate BEFORE TRUNCATE ON document_postings FOR EACH STATEMENT EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_relations_no_truncate BEFORE TRUNCATE ON document_relations FOR EACH STATEMENT EXECUTE FUNCTION reject_protected_change();

INSERT INTO document_migration_issues(kind,document_id,details)
 SELECT 'source_removal_uncertain',d.id,jsonb_build_object('planned_removal_on',d.withdraw_on,'note','Generic source deadline is not evidence of actual removal')
 FROM documents d WHERE d.board_status IN ('archived','withdrawn') AND EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id AND p.origin='source' AND p.removed_at IS NULL AND p.removed_on IS NULL);
