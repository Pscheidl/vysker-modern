"""Persist publication recipients for the application mail worker."""
from datetime import datetime


def queue_publication(conn, timestamp, notice_id=None, document_id=None):
    """Snapshot active consent in the caller's publication transaction."""
    if (notice_id is None) == (document_id is None):
        raise ValueError('A publication must identify exactly one notice or document')
    conn.execute('''INSERT INTO publication_outbox
        (notice_id,document_id,subscriber_id,consent_id,created_at)
        SELECT %s,%s,s.id,c.id,%s FROM subscribers s
        JOIN subscription_consents c ON c.subscriber_id=s.id
        WHERE s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
          AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL
          AND c.superseded_at IS NULL
        ORDER BY s.id,c.id
        ON CONFLICT DO NOTHING''',
        (notice_id, document_id, int(datetime.fromisoformat(timestamp).timestamp())))
