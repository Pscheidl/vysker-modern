#![cfg(feature = "ssr")]
mod common;
use common::App;
use obecni_web::backend::{Backend, mail};
use time::OffsetDateTime;

#[tokio::test]
async fn smtp_failure_keeps_message_for_retry() {
    let app = App::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.state.config).clone();
    config.smtp_port = listener.local_addr().unwrap().port();
    drop(listener);
    let state = Backend::new(app.state.pool.clone(), config);
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&state, "retry@example.test", now)
        .await
        .unwrap();
    assert!(
        mail::deliver_one(&state, &mail::transport(&state).unwrap(), now)
            .await
            .unwrap()
    );
    let row: (i64, Option<i64>, i64, i64, bool) = sqlx::query_as(
        "SELECT attempts,sent_at,next_attempt_at,locked_until,body IS NOT NULL FROM mail_queue",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(row.0, 1);
    assert_eq!(row.1, None);
    assert!(row.2 > now.unix_timestamp());
    assert_eq!(row.3, 0);
    assert!(row.4);
    assert!(
        !mail::deliver_one(&state, &mail::transport(&state).unwrap(), now)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn expired_verification_is_cancelled_without_smtp_delivery() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "expired-mail@example.test", now)
        .await
        .unwrap();
    let expired = now + time::Duration::days(1);
    assert!(
        mail::deliver_one(&app.state, &mail::transport(&app.state).unwrap(), expired)
            .await
            .unwrap()
    );
    let row: (bool, Option<i64>, Option<String>) =
        sqlx::query_as("SELECT cancelled,sent_at,body FROM mail_queue")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(row.0);
    assert!(row.1.is_none());
    assert!(row.2.is_none());
}

#[tokio::test]
#[ignore = "vyžaduje Mailpit, spusťte scripts/test.sh --smtp"]
async fn mailpit_accepts_verification_email() {
    let app = App::new().await;
    let mut config = (*app.state.config).clone();
    config.smtp_host = std::env::var("TEST_SMTP_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    config.smtp_port = std::env::var("TEST_SMTP_PORT")
        .unwrap_or_else(|_| "1025".into())
        .parse()
        .unwrap();
    let state = Backend::new(app.state.pool.clone(), config);
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&state, "mailpit@example.test", now)
        .await
        .unwrap();
    assert!(
        mail::deliver_one(&state, &mail::transport(&state).unwrap(), now)
            .await
            .unwrap()
    );
    let row: (Option<i64>, Option<String>) = sqlx::query_as("SELECT sent_at,body FROM mail_queue")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(row.0.is_some(), "Mailpit nepřijal zprávu");
    assert!(row.1.is_none(), "po odeslání se token z fronty odstraní");
}

#[tokio::test]
async fn concurrent_workers_claim_a_pending_message_only_once() {
    let app = App::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.state.config).clone();
    config.smtp_port = listener.local_addr().unwrap().port();
    drop(listener);
    let state = Backend::new(app.state.pool.clone(), config);
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&state, "parallel@example.test", now)
        .await
        .unwrap();
    let smtp = mail::transport(&state).unwrap();
    let (a, b) = tokio::join!(
        mail::deliver_one(&state, &smtp, now),
        mail::deliver_one(&state, &smtp, now)
    );
    assert_ne!(a.unwrap(), b.unwrap());
    let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM mail_queue")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1);
}
