-- One unreleased migration from deployed schema 0028 to the canonical registry.
-- All conversion steps are atomic in the existing SQLx migration transaction.

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
UPDATE documents d SET catalog_date=coalesce(document_date,source_published_on,CASE WHEN NOT EXISTS(SELECT 1 FROM legacy_sources s WHERE s.document_id=d.id) THEN (published_at::timestamptz AT TIME ZONE 'Europe/Prague')::date END);
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
 document_id BIGINT REFERENCES documents(id),
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

-- Change only the label. Category IDs, document links and subscriber choices survive.
-- Conflicting existing labels are kept separate for explicit operator review.
INSERT INTO document_migration_issues(kind,details)
 SELECT 'category_rename_conflict',jsonb_build_object('old_category_id',old.id,'existing_category_id',target.id,
   'old_name',old.name,'requested_name',target.name,'note','Both category identities were preserved. No documents or subscriptions were merged.')
 FROM categories old CROSS JOIN categories target
 WHERE old.name='Zastupitelstvo' AND target.name='Zasedání zastupitelstva';
UPDATE categories SET name='Zasedání zastupitelstva',version=version+1
 WHERE name='Zastupitelstvo'
 AND NOT EXISTS(SELECT 1 FROM categories WHERE name='Zasedání zastupitelstva');

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


-- Import provenance is not local publication evidence. Source policies default
-- to proposals and never gain authority over local visibility decisions.
CREATE TABLE document_import_sources (
    id TEXT PRIMARY KEY,
    origin_url TEXT NOT NULL UNIQUE,
    mode TEXT NOT NULL DEFAULT 'proposals' CHECK (mode IN ('one_time','mirror','proposals','disabled')),
    allow_publication BOOLEAN NOT NULL DEFAULT FALSE,
    allow_operational BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO document_import_sources(id,origin_url,mode)
VALUES ('vismo:vysker.cz','https://vysker.cz','proposals');
ALTER TABLE legacy_sources ADD COLUMN source_id TEXT NOT NULL DEFAULT 'vismo:vysker.cz'
    REFERENCES document_import_sources(id);
CREATE UNIQUE INDEX legacy_sources_identity ON legacy_sources(source_id,source_key);
CREATE TABLE document_source_observations (
    source_key TEXT PRIMARY KEY REFERENCES legacy_sources(source_key),
    last_observed_at TIMESTAMPTZ NOT NULL,
    last_fingerprint TEXT NOT NULL
);
CREATE TABLE document_source_aliases (
    source_id TEXT NOT NULL REFERENCES document_import_sources(id),
    source_url TEXT NOT NULL,
    source_key TEXT NOT NULL REFERENCES legacy_sources(source_key),
    PRIMARY KEY (source_id,source_url)
);
INSERT INTO document_source_aliases SELECT source_id,source_url,source_key FROM legacy_sources;
CREATE TABLE document_import_runs (
    id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES document_import_sources(id),
    mode TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    status TEXT NOT NULL CHECK (status IN ('running','applied','failed','dry_run')),
    summary JSONB NOT NULL DEFAULT '{}'
);
CREATE TABLE document_import_revisions (
    id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    source_key TEXT NOT NULL REFERENCES legacy_sources(source_key),
    document_id BIGINT REFERENCES documents(id),
    fingerprint TEXT NOT NULL CHECK (fingerprint ~ '^[a-f0-9]{64}$'),
    content_fingerprint TEXT NOT NULL CHECK (content_fingerprint ~ '^[a-f0-9]{64}$'),
    proposal JSONB NOT NULL,
    source_data BYTEA NOT NULL,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','rejected','accepted')),
    decided_at TIMESTAMPTZ,
    decided_by BIGINT REFERENCES administrators(id),
    decision_reason TEXT,
    accepted_document_id BIGINT REFERENCES documents(id),
    UNIQUE (source_key,fingerprint),
    CHECK ((state='pending' AND decided_at IS NULL AND accepted_document_id IS NULL)
        OR (state='rejected' AND decided_at IS NOT NULL AND accepted_document_id IS NULL)
        OR (state='accepted' AND decided_at IS NOT NULL AND accepted_document_id IS NOT NULL))
);
CREATE INDEX document_import_revisions_pending ON document_import_revisions(first_seen_at,id)
    WHERE state='pending';
CREATE INDEX document_import_revisions_document ON document_import_revisions(document_id,id);
CREATE INDEX document_import_revisions_accepted ON document_import_revisions(source_key,decided_at DESC,id DESC)
    WHERE state='accepted';
CREATE TABLE document_import_revision_files (
    revision_id BIGINT NOT NULL REFERENCES document_import_revisions(id),
    position INTEGER NOT NULL CHECK (position>=0),
    name TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[a-f0-9]{64}$'),
    data BYTEA NOT NULL,
    is_public BOOLEAN NOT NULL DEFAULT TRUE,
    PRIMARY KEY (revision_id,position),
    CHECK (size_bytes=octet_length(data) AND sha256=encode(sha256(data),'hex'))
);
CREATE FUNCTION protect_document_import_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP<>'UPDATE' THEN
        RAISE EXCEPTION 'Import evidence cannot be deleted';
    END IF;
    IF (to_jsonb(NEW)-ARRAY['state','decided_at','decided_by','decision_reason','accepted_document_id'])
        IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['state','decided_at','decided_by','decision_reason','accepted_document_id'])
        OR OLD.state<>'pending' OR NEW.state='pending' THEN
        RAISE EXCEPTION 'Import evidence is immutable and decisions are final';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER document_import_revisions_immutable BEFORE UPDATE OR DELETE ON document_import_revisions
    FOR EACH ROW EXECUTE FUNCTION protect_document_import_revision();
CREATE TRIGGER document_import_revisions_no_truncate BEFORE TRUNCATE ON document_import_revisions
    FOR EACH STATEMENT EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_import_revision_files_immutable BEFORE UPDATE OR DELETE ON document_import_revision_files
    FOR EACH ROW EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER document_import_revision_files_no_truncate BEFORE TRUNCATE ON document_import_revision_files
    FOR EACH STATEMENT EXECUTE FUNCTION reject_protected_change();
-- Retain proposals already observed before this migration. The old table remains
-- a compatibility projection for existing operator exports, not the revision log.
INSERT INTO document_import_revisions(source_key,document_id,fingerprint,content_fingerprint,
    proposal,source_data,first_seen_at,state,decided_at,decision_reason)
SELECT r.source_key,COALESCE(s.document_id,s.notice_id),r.fingerprint,r.fingerprint,
    CASE WHEN COALESCE(s.document_id,s.notice_id) IS NOT NULL AND NOT r.proposal ? 'mime'
        THEN r.proposal || '{"incomplete":true,"legacy_migrated_revision":true}'::jsonb
        ELSE r.proposal || jsonb_build_object('legacy_migrated_revision',TRUE,'replacement_attachment_id',s.attachment_id) END,r.source_data,r.first_seen_at::timestamptz,
    CASE WHEN r.reviewed_at IS NULL THEN 'pending' ELSE 'rejected' END,
    r.reviewed_at::timestamptz,
    CASE WHEN r.reviewed_at IS NOT NULL THEN 'Legacy review acknowledged before canonical migration' END
FROM legacy_sync_reviews r JOIN legacy_sources s USING (source_key);

INSERT INTO document_import_revision_files(revision_id,position,name,content_type,size_bytes,sha256,data,is_public)
SELECT r.id,0,coalesce(r.proposal->>'name','source-file'),r.proposal->>'mime',
    octet_length(r.source_data),encode(sha256(r.source_data),'hex'),r.source_data,
    coalesce(a.is_public,FALSE)
FROM document_import_revisions r JOIN legacy_sources s USING(source_key)
LEFT JOIN attachments a ON a.id=s.attachment_id WHERE r.proposal ? 'mime';

CREATE FUNCTION document_decide_revision(p_revision_id BIGINT,p_decision TEXT,p_reason TEXT,p_actor BIGINT)
RETURNS BIGINT LANGUAGE plpgsql AS $$
DECLARE candidate document_import_revisions; original documents; new_id BIGINT; offset_order BIGINT;
        blocked BOOLEAN; replacement BIGINT;
BEGIN
    IF p_decision NOT IN ('accept','reject') OR nullif(btrim(p_reason),'') IS NULL THEN
        RAISE EXCEPTION 'Decision and nonempty internal reason are required' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard';
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write',0));
    SELECT * INTO candidate FROM document_import_revisions WHERE id=p_revision_id FOR UPDATE;
    IF NOT FOUND OR candidate.state<>'pending' THEN
        RAISE EXCEPTION 'Pending revision not found or already decided' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard';
    END IF;
    IF p_decision='accept' THEN
        IF candidate.document_id IS NULL THEN
            RAISE EXCEPTION 'Use the page or calendar review workflow for this source proposal' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard';
        END IF;
        IF coalesce((candidate.proposal->>'incomplete')::boolean,FALSE) THEN
            RAISE EXCEPTION 'Incomplete source revision cannot be accepted' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard';
        END IF;
        SELECT * INTO original FROM documents WHERE id=candidate.document_id FOR UPDATE;
        SELECT original.access_blocked OR EXISTS(SELECT 1 FROM document_import_revisions r
            JOIN documents d ON d.id=r.accepted_document_id
            WHERE r.document_id=original.id AND d.access_blocked) INTO blocked;
        INSERT INTO documents(title,description,reference_number,issuer,category_id,status,created_at,
            source_published_on,document_date,public_access,access_blocked,attachments_public)
        VALUES(left(coalesce(candidate.proposal->>'title',original.title),300),
            coalesce(candidate.proposal->>'content',original.description),
            original.reference_number,original.issuer,original.category_id,'draft',
            to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
            NULL,NULL,FALSE,blocked,original.attachments_public)
        RETURNING id INTO new_id;
        replacement=(candidate.proposal->>'replacement_attachment_id')::bigint;
        IF NOT coalesce((candidate.proposal->>'replaces_attachment_list')::boolean,FALSE) THEN
            INSERT INTO attachments(document_id,name,content_type,size_bytes,sort_order,data,sha256,is_public)
            SELECT new_id,name,content_type,size_bytes,ordinal,data,sha256,is_public FROM (
                SELECT a.*,row_number() OVER(ORDER BY sort_order,id)-1 AS ordinal FROM attachments a
                WHERE document_id=original.id AND data IS NOT NULL AND removed_at IS NULL
            ) ordered WHERE id IS DISTINCT FROM replacement;
        END IF;
        SELECT coalesce(max(sort_order)+1,0) INTO offset_order FROM attachments WHERE document_id=new_id;
        IF replacement IS NOT NULL AND NOT coalesce((candidate.proposal->>'replaces_attachment_list')::boolean,FALSE) THEN
            SELECT ordinal INTO offset_order FROM (
                SELECT id,row_number() OVER(ORDER BY sort_order,id)-1 AS ordinal FROM attachments
                WHERE document_id=original.id AND data IS NOT NULL AND removed_at IS NULL
            ) ordered WHERE id=replacement;
            offset_order=coalesce(offset_order,0);
        END IF;
        INSERT INTO attachments(document_id,name,content_type,size_bytes,sort_order,data,sha256,is_public)
        SELECT new_id,name,content_type,size_bytes,position+offset_order,data,sha256,is_public
            FROM document_import_revision_files WHERE revision_id=candidate.id ORDER BY position;
        INSERT INTO document_relations(original_id,related_id,kind,actor_id,reason)
            VALUES(original.id,new_id,'source_change',p_actor,p_reason);
        INSERT INTO document_events(document_id,actor_id,kind,reason,payload)
            VALUES(new_id,p_actor,'source_revision_accepted',p_reason,
                jsonb_build_object('revision_id',candidate.id,'source_document_id',original.id));
    END IF;
    UPDATE document_import_revisions SET state=CASE WHEN p_decision='accept' THEN 'accepted' ELSE 'rejected' END,
        decided_at=now(),decided_by=p_actor,decision_reason=p_reason,accepted_document_id=new_id WHERE id=candidate.id;
    UPDATE legacy_sync_reviews SET reviewed_at=to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
        WHERE source_key=candidate.source_key AND fingerprint=candidate.fingerprint;
    INSERT INTO audit_log(actor_id,operation,entity_type,entity_id,details)
        VALUES(p_actor,CASE WHEN p_decision='accept' THEN 'source_revision_accepted' ELSE 'source_revision_rejected' END,
            'document',candidate.document_id,jsonb_build_object('revision_id',candidate.id,
                'new_document_id',new_id,'reason',p_reason)::text);
    RETURN new_id;
END $$;

CREATE FUNCTION audit_document_import_source() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id OR NEW.origin_url IS DISTINCT FROM OLD.origin_url THEN
        RAISE EXCEPTION 'A source identity cannot be repurposed' USING ERRCODE='23514', CONSTRAINT='document_lifecycle_guard';
    END IF;
    IF ROW(NEW.mode,NEW.allow_publication,NEW.allow_operational)
        IS DISTINCT FROM ROW(OLD.mode,OLD.allow_publication,OLD.allow_operational) THEN
        INSERT INTO audit_log(operation,entity_type,details)
        VALUES('import_source_policy_changed','import_source',
            jsonb_build_object('source_id',OLD.id,'before',to_jsonb(OLD),'after',to_jsonb(NEW),
                'database_actor',current_user)::text);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER document_import_sources_audit BEFORE UPDATE ON document_import_sources
    FOR EACH ROW EXECUTE FUNCTION audit_document_import_source();


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
CREATE INDEX documents_public_archive ON documents(catalog_date DESC NULLS LAST,id DESC)
 WHERE public_access AND NOT access_blocked AND ((board_status IS NULL AND status='archived') OR has_removed_posting)
 AND NOT has_current_posting AND board_status IS DISTINCT FROM 'scheduled';
CREATE FUNCTION validate_document_posting_projection() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 NEW.has_current_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=NEW.id AND p.certainty='documented' AND p.removed_at IS NULL AND p.removed_on IS NULL);
 NEW.has_removed_posting=EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=NEW.id AND p.certainty='documented' AND (p.removed_at IS NOT NULL OR p.removed_on IS NOT NULL));
 RETURN NEW;
END $$;
CREATE TRIGGER documents_posting_projection BEFORE INSERT OR UPDATE OF has_current_posting,has_removed_posting ON documents
 FOR EACH ROW EXECUTE FUNCTION validate_document_posting_projection();


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
  IF NEW.status='draft' AND NEW.status IS DISTINCT FROM OLD.status AND NEW.public_access IS NOT DISTINCT FROM OLD.public_access THEN NEW.public_access=FALSE; END IF;
 ELSE NEW.public_access=NEW.public_access OR (NEW.status='published' AND NOT NEW.access_blocked);
 END IF;
 NEW.catalog_date=coalesce(NEW.document_date,NEW.source_published_on,CASE WHEN NOT EXISTS(SELECT 1 FROM legacy_sources s WHERE s.document_id=NEW.id) THEN (NEW.published_at::timestamptz AT TIME ZONE 'Europe/Prague')::date END);
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

CREATE TRIGGER documents_guard BEFORE INSERT OR UPDATE OR DELETE ON documents FOR EACH ROW EXECUTE FUNCTION protect_document();
CREATE TRIGGER documents_no_truncate BEFORE TRUNCATE ON documents EXECUTE FUNCTION reject_protected_change();
CREATE TRIGGER attachments_document_guard BEFORE INSERT OR UPDATE ON attachments FOR EACH ROW EXECUTE FUNCTION protect_document_attachment();


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
