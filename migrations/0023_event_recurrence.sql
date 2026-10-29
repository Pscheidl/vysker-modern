-- Store one stable series, with an optional inclusive last allowed start date.
ALTER TABLE events ADD COLUMN repeat_every_14_days BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE events ADD COLUMN repeat_until DATE;
ALTER TABLE events ADD CONSTRAINT events_recurrence_until CHECK (
    repeat_until IS NULL OR (repeat_every_14_days AND repeat_until >= (starts_at AT TIME ZONE 'Europe/Prague')::DATE)
);
ALTER TABLE calendar_withdrawals ADD COLUMN repeat_every_14_days BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE calendar_withdrawals ADD COLUMN repeat_until DATE;

-- Include recurrence revisions in direct importer updates and preserve only the
-- former public recurrence metadata when a whole series must be withdrawn.
CREATE OR REPLACE FUNCTION remember_calendar_withdrawal() RETURNS trigger LANGUAGE plpgsql AS $$
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
               NEW.end_date_known,NEW.published,NEW.cancelled,NEW.repeat_every_14_days,NEW.repeat_until)
           IS DISTINCT FROM
           ROW(OLD.title,OLD.description,OLD.location,OLD.category,OLD.all_day,
               OLD.starts_at,OLD.ends_at,OLD.start_time_known,OLD.end_time_known,
               OLD.end_date_known,OLD.published,OLD.cancelled,OLD.repeat_every_14_days,OLD.repeat_until)
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
            (calendar_uid,category,starts_at,ends_at,all_day,start_time_known,end_time_known,end_date_known,version,updated_at,repeat_every_14_days,repeat_until)
        VALUES (OLD.calendar_uid,OLD.category,OLD.starts_at,OLD.ends_at,OLD.all_day,
                OLD.start_time_known,OLD.end_time_known,OLD.end_date_known,next_version,changed_at,OLD.repeat_every_14_days,OLD.repeat_until)
        ON CONFLICT (calendar_uid,category) DO UPDATE SET
            starts_at=EXCLUDED.starts_at,ends_at=EXCLUDED.ends_at,all_day=EXCLUDED.all_day,
            start_time_known=EXCLUDED.start_time_known,end_time_known=EXCLUDED.end_time_known,
            end_date_known=EXCLUDED.end_date_known,version=EXCLUDED.version,updated_at=EXCLUDED.updated_at,
            repeat_every_14_days=EXCLUDED.repeat_every_14_days,repeat_until=EXCLUDED.repeat_until;
        IF removed THEN
            INSERT INTO calendar_withdrawals
                (calendar_uid,category,starts_at,ends_at,all_day,start_time_known,end_time_known,end_date_known,version,updated_at,repeat_every_14_days,repeat_until)
            VALUES (OLD.calendar_uid,'*',OLD.starts_at,OLD.ends_at,OLD.all_day,
                    OLD.start_time_known,OLD.end_time_known,OLD.end_date_known,next_version,changed_at,OLD.repeat_every_14_days,OLD.repeat_until)
            ON CONFLICT (calendar_uid,category) DO UPDATE SET
                starts_at=EXCLUDED.starts_at,ends_at=EXCLUDED.ends_at,all_day=EXCLUDED.all_day,
                start_time_known=EXCLUDED.start_time_known,end_time_known=EXCLUDED.end_time_known,
                end_date_known=EXCLUDED.end_date_known,version=EXCLUDED.version,updated_at=EXCLUDED.updated_at,
            repeat_every_14_days=EXCLUDED.repeat_every_14_days,repeat_until=EXCLUDED.repeat_until;
        END IF;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
