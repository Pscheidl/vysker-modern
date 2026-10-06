-- Preserve delivery of all publications for existing subscribers and consents.
ALTER TABLE subscribers
    ADD COLUMN all_notice_categories BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN notice_category_ids BIGINT[] NOT NULL DEFAULT '{}',
    ADD COLUMN uncategorized_notices BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN documents BOOLEAN NOT NULL DEFAULT TRUE;

-- Pending selections belong to the exact consent being confirmed. An anonymous
-- subscription request must never change a verified subscriber's preferences.
ALTER TABLE subscription_consents
    ADD COLUMN all_notice_categories BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN notice_category_ids BIGINT[] NOT NULL DEFAULT '{}',
    ADD COLUMN uncategorized_notices BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN documents BOOLEAN NOT NULL DEFAULT TRUE;

CREATE OR REPLACE FUNCTION protect_consent_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.subscriber_id IS DISTINCT FROM OLD.subscriber_id
        OR NEW.notice_fingerprint IS DISTINCT FROM OLD.notice_fingerprint
        OR NEW.requested_at IS DISTINCT FROM OLD.requested_at
        OR NEW.all_notice_categories IS DISTINCT FROM OLD.all_notice_categories
        OR NEW.notice_category_ids IS DISTINCT FROM OLD.notice_category_ids
        OR NEW.uncategorized_notices IS DISTINCT FROM OLD.uncategorized_notices
        OR NEW.documents IS DISTINCT FROM OLD.documents
        OR (OLD.confirmed_at IS NOT NULL AND NEW.confirmed_at IS DISTINCT FROM OLD.confirmed_at)
        OR (OLD.withdrawn_at IS NOT NULL AND NEW.withdrawn_at IS DISTINCT FROM OLD.withdrawn_at)
        OR (OLD.superseded_at IS NOT NULL AND NEW.superseded_at IS DISTINCT FROM OLD.superseded_at)
    THEN RAISE EXCEPTION 'consent evidence is immutable'; END IF;
    RETURN NEW;
END;
$$;
