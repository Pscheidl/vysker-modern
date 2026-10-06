#![cfg(feature = "ssr")]
mod common;

use common::{App, body_json};
use obecni_web::backend::{Backend, mail, privacy, subscriptions};
use serde_json::json;
use time::{Duration, OffsetDateTime};

async fn subscriber(app: &App, now: OffsetDateTime) -> (i64, i64) {
    common::request_subscription(&app.state, "retention@example.test", now)
        .await
        .unwrap();
    let token = app.confirmation_token("retention@example.test").await;
    subscriptions::use_token(&app.state, &token, true, now)
        .await
        .unwrap();
    sqlx::query_as("SELECT s.id,c.id FROM subscribers s JOIN subscription_consents c ON c.subscriber_id=s.id WHERE s.email='retention@example.test' AND c.confirmed_at IS NOT NULL")
        .fetch_one(&app.state.pool).await.unwrap()
}

#[tokio::test]
async fn publication_survives_restart_and_retention_before_and_after_materialization() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    subscriber(&app, now).await;
    let notice = app.notice(json!({})).await;
    assert!(app.publish(notice).await.status().is_success());
    let old = now
        - Duration::days(
            i64::from(
                app.state
                    .config
                    .privacy
                    .as_ref()
                    .unwrap()
                    .retention
                    .mail_days,
            ) + 2,
        );
    // Persisted publication work can predate a long application outage.
    sqlx::query("UPDATE publication_outbox SET created_at=$1")
        .bind(old.unix_timestamp())
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
    let before = body_json(app.call("GET", "/api/v1/admin/overview", None, true).await).await;
    assert_eq!(before["pending_mail"], 1);

    let restarted = Backend::new(app.state.pool.clone(), (*app.state.config).clone());
    privacy::maintenance(&restarted, now).await.unwrap();
    assert!(mail::prepare_publications(&restarted, now).await.unwrap());
    let pending: (i64, i64, String, i64) = sqlx::query_as(
        "SELECT id,created_at,body,attempts FROM mail_queue WHERE purpose='document'",
    )
    .fetch_one(&restarted.pool)
    .await
    .unwrap();
    assert_eq!(pending.1, old.unix_timestamp());
    assert_eq!(pending.3, 0);
    privacy::maintenance(&restarted, now).await.unwrap();

    let restarted_again = Backend::new(restarted.pool.clone(), (*restarted.config).clone());
    assert!(
        !mail::prepare_publications(&restarted_again, now)
            .await
            .unwrap()
    );
    let preserved: (i64, i64, String, i64) = sqlx::query_as(
        "SELECT id,created_at,body,attempts FROM mail_queue WHERE purpose='document'",
    )
    .fetch_one(&restarted_again.pool)
    .await
    .unwrap();
    assert_eq!(preserved, pending);
    let after = body_json(app.call("GET", "/api/v1/admin/overview", None, true).await).await;
    assert_eq!(after["pending_mail"], 1);

    sqlx::query("UPDATE mail_queue SET sent_at=$1,body=NULL WHERE id=$2")
        .bind(now.unix_timestamp())
        .bind(pending.0)
        .execute(&restarted_again.pool)
        .await
        .unwrap();
    privacy::maintenance(&restarted_again, now).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&restarted_again.pool)
            .await
            .unwrap(),
        0
    );
    let completed = body_json(app.call("GET", "/api/v1/admin/overview", None, true).await).await;
    assert_eq!(completed["pending_mail"], 0);
}

#[tokio::test]
async fn retention_removes_completed_and_verification_mail_but_keeps_pending_and_leased_rows() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let (subscriber, consent) = subscriber(&app, now).await;
    let old = now - Duration::days(400);
    for (subject, purpose, sent, cancelled, leased) in [
        ("pending", "document", false, false, false),
        ("sent", "document", true, false, false),
        ("cancelled", "document", false, true, false),
        ("verification", "verification", false, false, false),
        ("leased", "document", true, false, true),
    ] {
        sqlx::query("INSERT INTO mail_queue(subscriber_id,consent_id,purpose,subject,body,created_at,next_attempt_at,sent_at,cancelled,locked_until) VALUES ($1,$2,$3,$4,'body',$5,$5,$6,$7,$8)")
            .bind(subscriber).bind(consent).bind(purpose).bind(subject)
            .bind(old.unix_timestamp()).bind(sent.then_some(old.unix_timestamp()))
            .bind(cancelled).bind(if leased { now.unix_timestamp() + 300 } else { 0 })
            .execute(&app.state.pool).await.unwrap();
    }
    privacy::maintenance(&app.state, now).await.unwrap();
    let retained: Vec<String> =
        sqlx::query_scalar("SELECT subject FROM mail_queue WHERE created_at=$1 ORDER BY subject")
            .bind(old.unix_timestamp())
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(retained, ["leased", "pending"]);
    privacy::maintenance(&app.state, now + Duration::minutes(6))
        .await
        .unwrap();
    let retained: Vec<String> =
        sqlx::query_scalar("SELECT subject FROM mail_queue WHERE created_at=$1 ORDER BY subject")
            .bind(old.unix_timestamp())
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(retained, ["pending"]);
}
