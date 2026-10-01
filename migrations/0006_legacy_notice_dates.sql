-- Historical navigation can identify a notice even when its posting date is absent.
-- Normal publication still requires a date, imported drafts/previews may keep it unknown.
ALTER TABLE notices ALTER COLUMN published_on DROP NOT NULL;
ALTER TABLE notices ADD CONSTRAINT notices_scheduled_date_known
    CHECK (status <> 'scheduled' OR published_on IS NOT NULL);
ALTER TABLE notices ADD CONSTRAINT notices_publication_date_known
    CHECK (published_at IS NULL OR published_on IS NOT NULL);
