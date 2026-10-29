-- Event categories are independent from official-notice categories.
ALTER TABLE events ADD COLUMN category TEXT NOT NULL DEFAULT 'other'
    CHECK (category IN ('other','waste','culture','council'));
ALTER TABLE events ADD COLUMN all_day BOOLEAN NOT NULL DEFAULT FALSE;
-- UUIDs survive domain changes and avoid collisions with other installations.
ALTER TABLE events ADD COLUMN calendar_uid TEXT NOT NULL DEFAULT (gen_random_uuid()::TEXT || '@obecni-web');
CREATE UNIQUE INDEX events_calendar_uid ON events(calendar_uid);

-- Keep only the public identity and former dates needed to cancel a subscription
-- entry. Never retain the removed title, description, place or private revisions.
CREATE TABLE calendar_withdrawals (
    calendar_uid TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('*','other','waste','culture','council')),
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL,
    all_day BOOLEAN NOT NULL,
    start_time_known BOOLEAN NOT NULL,
    end_time_known BOOLEAN NOT NULL,
    end_date_known BOOLEAN NOT NULL,
    version BIGINT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(calendar_uid,category)
);

CREATE FUNCTION remember_calendar_withdrawal() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    removed BOOLEAN;
    changed_at TEXT;
    next_version BIGINT;
BEGIN
    IF TG_OP='DELETE' THEN
        removed := TRUE;
        changed_at := to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"');
        next_version := OLD.version + 1;
    ELSE
        -- Import jobs can update event content directly. Advance their revision too.
        NEW.calendar_uid := OLD.calendar_uid;
        IF ROW(NEW.title,NEW.description,NEW.location,NEW.category,NEW.all_day,
               NEW.starts_at,NEW.ends_at,NEW.start_time_known,NEW.end_time_known,
               NEW.end_date_known,NEW.published,NEW.cancelled)
           IS DISTINCT FROM
           ROW(OLD.title,OLD.description,OLD.location,OLD.category,OLD.all_day,
               OLD.starts_at,OLD.ends_at,OLD.start_time_known,OLD.end_time_known,
               OLD.end_date_known,OLD.published,OLD.cancelled)
           AND NEW.version <= OLD.version THEN
            NEW.version := OLD.version + 1;
            NEW.updated_at := to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"');
        END IF;
        removed := NOT NEW.published;
        changed_at := NEW.updated_at;
        next_version := NEW.version;
    END IF;
    IF OLD.published AND (removed OR OLD.category IS DISTINCT FROM NEW.category) THEN
        INSERT INTO calendar_withdrawals
            (calendar_uid,category,starts_at,ends_at,all_day,start_time_known,end_time_known,end_date_known,version,updated_at)
        VALUES (OLD.calendar_uid,OLD.category,OLD.starts_at,OLD.ends_at,OLD.all_day,
                OLD.start_time_known,OLD.end_time_known,OLD.end_date_known,next_version,changed_at)
        ON CONFLICT (calendar_uid,category) DO UPDATE SET
            starts_at=EXCLUDED.starts_at,ends_at=EXCLUDED.ends_at,all_day=EXCLUDED.all_day,
            start_time_known=EXCLUDED.start_time_known,end_time_known=EXCLUDED.end_time_known,
            end_date_known=EXCLUDED.end_date_known,version=EXCLUDED.version,updated_at=EXCLUDED.updated_at;
        IF removed THEN
            INSERT INTO calendar_withdrawals
                (calendar_uid,category,starts_at,ends_at,all_day,start_time_known,end_time_known,end_date_known,version,updated_at)
            VALUES (OLD.calendar_uid,'*',OLD.starts_at,OLD.ends_at,OLD.all_day,
                    OLD.start_time_known,OLD.end_time_known,OLD.end_date_known,next_version,changed_at)
            ON CONFLICT (calendar_uid,category) DO UPDATE SET
                starts_at=EXCLUDED.starts_at,ends_at=EXCLUDED.ends_at,all_day=EXCLUDED.all_day,
                start_time_known=EXCLUDED.start_time_known,end_time_known=EXCLUDED.end_time_known,
                end_date_known=EXCLUDED.end_date_known,version=EXCLUDED.version,updated_at=EXCLUDED.updated_at;
        END IF;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER events_calendar_withdrawal BEFORE UPDATE OR DELETE ON events
    FOR EACH ROW EXECUTE FUNCTION remember_calendar_withdrawal();
