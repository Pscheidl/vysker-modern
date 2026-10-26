#![cfg(feature = "ssr")]
mod common;

use common::App;
use obecni_web::backend::{mail, subscriptions};
use std::time::Instant;
use time::OffsetDateTime;

/// Synthetic preparation workload only. No SMTP, production data or time limit.
#[tokio::test]
#[ignore = "synthetic digest workload, run explicitly with --ignored --nocapture"]
async fn one_thousand_recipients_receive_ten_documents_in_one_digest_each() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let cutoff =
        OffsetDateTime::from_unix_timestamp((now.unix_timestamp().div_euclid(3600) + 1) * 3600)
            .unwrap();

    // Obtain one real, confirmed consent and copy its synthetic evidence in bulk.
    // Account authentication is exercised once rather than 1,000 times.
    common::request_subscription(&app.state, "load-template@example.test", now)
        .await
        .unwrap();
    let token = app.confirmation_token("load-template@example.test").await;
    subscriptions::use_token(&app.state, &token, true, now)
        .await
        .unwrap();
    let consent: i64 = sqlx::query_scalar(
        "SELECT c.id FROM subscription_consents c
         JOIN subscribers s ON s.id=c.subscriber_id
         WHERE s.email='load-template@example.test' AND c.confirmed_at IS NOT NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    let inserted = sqlx::query(
        "WITH inserted AS (
             INSERT INTO subscribers(email,verified_at,retention_started_at)
             SELECT concat('load-',n,'@example.test'),$1,$1 FROM generate_series(1,999) n
             RETURNING id
         )
         INSERT INTO subscription_consents(
             subscriber_id,notice_fingerprint,requested_at,confirmed_at,
             all_notice_categories,notice_category_ids,uncategorized_notices,documents)
         SELECT inserted.id,c.notice_fingerprint,c.requested_at,c.confirmed_at,
             c.all_notice_categories,c.notice_category_ids,c.uncategorized_notices,c.documents
         FROM inserted CROSS JOIN subscription_consents c WHERE c.id=$2",
    )
    .bind(now.unix_timestamp())
    .bind(consent)
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(inserted.rows_affected(), 999);
    sqlx::query(
        "INSERT INTO documents(title,status,created_at,published_at)
         SELECT concat('Load document ',n),'published','2026-01-01T00:00:00Z',
             '2026-01-01T00:00:00Z' FROM generate_series(1,10) n",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let queued = sqlx::query(
        "INSERT INTO publication_outbox(document_id,subscriber_id,consent_id,created_at)
         SELECT d.id,s.id,c.id,$1 FROM documents d CROSS JOIN subscribers s
         JOIN subscription_consents c ON c.subscriber_id=s.id
         WHERE c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL
             AND c.superseded_at IS NULL AND s.verified_at IS NOT NULL
             AND s.unsubscribed_at IS NULL",
    )
    .bind(cutoff.unix_timestamp() - 1)
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(queued.rows_affected(), 10_000);

    let started = Instant::now();
    let mut prepared = 0;
    while mail::prepare_publications(&app.state, cutoff)
        .await
        .unwrap()
    {
        prepared += 1;
        assert!(prepared <= 1_000, "Preparation must make bounded progress");
    }
    let elapsed = started.elapsed();
    assert_eq!(prepared, 1_000);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM publication_outbox),
            (SELECT count(*) FROM mail_queue WHERE purpose='document'),
            (SELECT count(*) FROM mail_publication_items)",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(counts, (0, 1_000, 10_000));
    let incomplete: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (
             SELECT m.id FROM mail_queue m LEFT JOIN mail_publication_items i ON i.mail_id=m.id
             WHERE m.purpose='document' GROUP BY m.id HAVING count(i.id)<>10
         ) incomplete",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(incomplete, 0);
    let recipients: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT subscriber_id) FROM mail_queue WHERE purpose='document'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(recipients, 1_000);
    assert!(
        !mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    eprintln!(
        "Digest preparation only: 1,000 recipients, 10 documents, 10,000 outbox rows -> \
         1,000 emails and 10,000 item records in {} ms. SMTP delivery is not measured.",
        elapsed.as_millis()
    );
}
