-- Enable the new calendar topic for current subscribers without changing their
-- other selections or rewriting historical consent evidence.
ALTER TABLE subscribers ADD COLUMN events BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE subscription_consents ADD COLUMN events BOOLEAN;
ALTER TABLE subscription_consents ALTER COLUMN events SET DEFAULT TRUE;

-- Extend only unchanged defaults in the current policy. Custom wording, other
-- policy fields and immutable historical consent snapshots remain untouched.
UPDATE privacy_settings SET policy_json=jsonb_set(policy_json::jsonb,'{consent_text}',
    to_jsonb('Přihlášením žádáte o zasílání nových dokumentů včetně úřední desky a nově oznámených akcí na zadaný e-mail. Odhlásit se můžete kdykoli odkazem v každé zprávě.'::TEXT))::TEXT
    WHERE policy_json::jsonb->>'consent_text'='Přihlášením žádáte o zasílání nových dokumentů včetně úřední desky na zadaný e-mail. Odhlásit se můžete kdykoli odkazem v každé zprávě.';
UPDATE privacy_settings SET policy_json=jsonb_set(policy_json::jsonb,'{newsletter_terms}',
    to_jsonb($new_terms$Na základě dobrovolného souhlasu podle čl. 6 odst. 1 písm. a) GDPR používáme e-mailovou adresu k zasílání nových dokumentů včetně úřední desky a nově oznámených akcí. Evidujeme přesné znění souhlasu a čas žádosti, potvrzení a případného odvolání. Bez poskytnutí adresy nelze odběr zajistit, používání ostatních částí webu tím není omezeno.

Souhlas kdykoli odvoláte odkazem v každé novince bez přihlášení. Odvolání nemá vliv na zákonnost předchozího zpracování.

Vyhrazujeme si právo tyto podmínky změnit. Aktuální znění je vždy dostupné na této stránce.$new_terms$::TEXT))::TEXT
    WHERE policy_json::jsonb->>'newsletter_terms'=$old_terms$Na základě dobrovolného souhlasu podle čl. 6 odst. 1 písm. a) GDPR používáme e-mailovou adresu k zasílání nových dokumentů včetně úřední desky. Evidujeme přesné znění souhlasu a čas žádosti, potvrzení a případného odvolání. Bez poskytnutí adresy nelze odběr zajistit, používání ostatních částí webu tím není omezeno.

Souhlas kdykoli odvoláte odkazem v každé novince bez přihlášení. Odvolání nemá vliv na zákonnost předchozího zpracování.

Vyhrazujeme si právo tyto podmínky změnit. Aktuální znění je vždy dostupné na této stránce.$old_terms$;

-- Published history has already been announced outside this email topic. Record
-- that boundary without creating mail, including past, cancelled or withdrawn
-- events. Calendar withdrawal identities distinguish them from private drafts.
ALTER TABLE events ADD COLUMN email_announced_at BIGINT;
UPDATE events e SET email_announced_at=extract(epoch FROM statement_timestamp())::BIGINT
    WHERE e.published OR EXISTS (
        SELECT 1 FROM calendar_withdrawals w WHERE w.calendar_uid=e.calendar_uid
    );

-- Import replacement can remove an event and its provenance, then restore it
-- later with a new event ID. Keep announcement identity outside both lifetimes.
CREATE TABLE legacy_event_announcements (
    source_key TEXT PRIMARY KEY,
    announced_at BIGINT NOT NULL
);
INSERT INTO legacy_event_announcements(source_key,announced_at)
    SELECT s.source_key,coalesce(e.email_announced_at,extract(epoch FROM statement_timestamp())::BIGINT)
    FROM legacy_sources s LEFT JOIN events e ON e.id=s.event_id
    WHERE s.event_id IS NOT NULL AND (e.email_announced_at IS NOT NULL OR e.id IS NULL);

CREATE OR REPLACE FUNCTION protect_consent_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.subscriber_id IS DISTINCT FROM OLD.subscriber_id
        OR NEW.notice_fingerprint IS DISTINCT FROM OLD.notice_fingerprint
        OR NEW.requested_at IS DISTINCT FROM OLD.requested_at
        OR NEW.all_notice_categories IS DISTINCT FROM OLD.all_notice_categories
        OR NEW.notice_category_ids IS DISTINCT FROM OLD.notice_category_ids
        OR NEW.uncategorized_notices IS DISTINCT FROM OLD.uncategorized_notices
        OR NEW.documents IS DISTINCT FROM OLD.documents
        OR NEW.events IS DISTINCT FROM OLD.events
        OR (OLD.confirmed_at IS NOT NULL AND NEW.confirmed_at IS DISTINCT FROM OLD.confirmed_at)
        OR (OLD.withdrawn_at IS NOT NULL AND NEW.withdrawn_at IS DISTINCT FROM OLD.withdrawn_at)
        OR (OLD.superseded_at IS NOT NULL AND NEW.superseded_at IS DISTINCT FROM OLD.superseded_at)
    THEN RAISE EXCEPTION 'consent evidence is immutable'; END IF;
    RETURN NEW;
END;
$$;

-- Keep the same durable queue and digest memberships. Old public events are not
-- backfilled, and each newly announced event can belong to one digest per person.
ALTER TABLE publication_outbox
    ADD COLUMN event_id BIGINT REFERENCES events(id) ON DELETE CASCADE,
    DROP CONSTRAINT publication_outbox_check,
    ADD CONSTRAINT publication_outbox_one_target
        CHECK (num_nonnulls(notice_id,document_id,event_id)=1),
    ADD CONSTRAINT publication_outbox_event_recipient UNIQUE (event_id,subscriber_id);
ALTER TABLE mail_publication_items
    ADD COLUMN event_id BIGINT REFERENCES events(id) ON DELETE CASCADE,
    DROP CONSTRAINT mail_publication_items_check,
    ADD CONSTRAINT mail_publication_items_one_target
        CHECK (num_nonnulls(notice_id,document_id,event_id)=1),
    ADD CONSTRAINT mail_publication_items_event_recipient UNIQUE (event_id,subscriber_id);

CREATE FUNCTION subscription_matches_publication(
    subscriber BIGINT, notice BIGINT, document BIGINT, event BIGINT
) RETURNS BOOLEAN
LANGUAGE SQL STABLE
AS $$
    SELECT num_nonnulls(notice,document,event)=1 AND EXISTS (
        SELECT 1 FROM subscribers s
        WHERE s.id=subscriber
          AND (
            (notice IS NOT NULL AND EXISTS (
                SELECT 1 FROM notices n WHERE n.id=notice
                  AND CASE WHEN n.category_id IS NULL
                      THEN s.uncategorized_notices
                      ELSE s.all_notice_categories OR n.category_id=ANY(s.notice_category_ids)
                  END
            ))
            OR (document IS NOT NULL AND s.documents
                AND EXISTS (SELECT 1 FROM documents d WHERE d.id=document))
            OR (event IS NOT NULL AND s.events
                AND EXISTS (SELECT 1 FROM events e WHERE e.id=event AND e.published AND NOT e.cancelled))
          )
    )
$$;

-- Native publishers and Python imports using the established signature keep
-- their exact behavior while event publishers use the four-argument overload.
CREATE OR REPLACE FUNCTION subscription_matches_publication(
    subscriber BIGINT, notice BIGINT, document BIGINT
) RETURNS BOOLEAN
LANGUAGE SQL STABLE
AS $$
    SELECT subscription_matches_publication(subscriber,notice,document,NULL::BIGINT)
$$;
