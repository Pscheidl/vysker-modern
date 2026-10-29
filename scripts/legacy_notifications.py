"""Persist publication recipients for the application mail worker."""
from datetime import datetime


def queue_publication(conn, timestamp, notice_id=None, document_id=None, event_id=None):
    """Snapshot active consent in the caller's publication transaction."""
    if sum(value is not None for value in (notice_id, document_id, event_id)) != 1:
        raise ValueError('A publication must identify exactly one notice, document or event')
    if event_id is not None:
        conn.execute('''INSERT INTO publication_outbox
            (event_id,subscriber_id,consent_id,created_at)
            SELECT %s,s.id,c.id,%s FROM subscribers s
            JOIN subscription_consents c ON c.subscriber_id=s.id
            WHERE s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
              AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL
              AND c.superseded_at IS NULL
              AND subscription_matches_publication(s.id,NULL::BIGINT,NULL::BIGINT,%s)
            ORDER BY s.id,c.id
            ON CONFLICT DO NOTHING''',
            (event_id, int(datetime.fromisoformat(timestamp).timestamp()), event_id))
        return
    conn.execute('''INSERT INTO publication_outbox
        (notice_id,document_id,subscriber_id,consent_id,created_at)
        SELECT %s,%s,s.id,c.id,%s FROM subscribers s
        JOIN subscription_consents c ON c.subscriber_id=s.id
        WHERE s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
          AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL
          AND c.superseded_at IS NULL
          AND subscription_matches_publication(s.id,%s,%s)
        ORDER BY s.id,c.id
        ON CONFLICT DO NOTHING''',
        (notice_id, document_id, int(datetime.fromisoformat(timestamp).timestamp()),
         notice_id, document_id))
