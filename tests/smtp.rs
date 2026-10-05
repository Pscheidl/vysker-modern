#![cfg(feature = "ssr")]
mod common;
use common::App;
use obecni_web::backend::{Backend, mail, subscriptions};
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

async fn confirmed_subscriber(app: &App, email: &str) -> (i64, i64) {
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let token = app.confirmation_token(email).await;
    subscriptions::use_token(&app.state, &token, true, now)
        .await
        .unwrap();
    sqlx::query_as("SELECT s.id,c.id FROM subscribers s JOIN subscription_consents c ON c.subscriber_id=s.id WHERE s.email=$1 AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL")
        .bind(email).fetch_one(&app.state.pool).await.unwrap()
}

async fn unavailable_smtp(app: &App) -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.state.config).clone();
    config.smtp_port = listener.local_addr().unwrap().port();
    drop(listener);
    Backend::new(app.state.pool.clone(), config)
}

async fn imported_notice(app: &App, recipient: (i64, i64), now: OffsetDateTime) -> i64 {
    let id = sqlx::query_scalar::<_, i64>("INSERT INTO notices(title,status,review_json) VALUES ('Interní název','archived','{\"archive_title\":\"Veřejný archivní název\"}') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    queue_import(app, id, recipient, now).await;
    id
}

async fn queue_import(app: &App, notice_id: i64, recipient: (i64, i64), now: OffsetDateTime) {
    sqlx::query("INSERT INTO publication_outbox(notice_id,subscriber_id,consent_id,created_at) VALUES ($1,$2,$3,$4)")
        .bind(notice_id).bind(recipient.0).bind(recipient.1).bind(now.unix_timestamp())
        .execute(&app.state.pool).await.unwrap();
}

#[tokio::test]
async fn archived_import_uses_original_audience_public_title_and_retry_queue_once() {
    let app = App::new().await;
    let original = confirmed_subscriber(&app, "original@example.test").await;
    let now = OffsetDateTime::now_utc();
    let id = imported_notice(&app, original, now).await;
    confirmed_subscriber(&app, "later@example.test").await;
    let state = unavailable_smtp(&app).await;
    let smtp = mail::transport(&state).unwrap();
    let (a, b) = tokio::join!(
        mail::deliver_one(&state, &smtp, now),
        mail::deliver_one(&state, &smtp, now)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(a || b);
    let messages: Vec<(i64, i64, String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT subscriber_id,consent_id,subject,body,attempts,sent_at FROM mail_queue WHERE purpose='document'")
        .fetch_all(&state.pool).await.unwrap();
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!((message.0, message.1), original);
    assert_eq!(message.2, "Vyskeř: Veřejný archivní název");
    assert!(!message.3.contains("Interní název"));
    assert!(
        message
            .3
            .contains(&format!("http://localhost:3000/uredni-deska/{id}"))
    );
    assert_eq!(message.4, 1);
    assert_eq!(message.5, None);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        0
    );

    // Even a repeated handoff shares the native publication deduplication key.
    queue_import(&app, id, original, now).await;
    assert!(mail::deliver_one(&state, &smtp, now).await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM subscription_tokens WHERE purpose='unsubscribe'"
        )
        .fetch_one(&state.pool)
        .await
        .unwrap(),
        1
    );
    let token = common::token_from_body(&message.3);
    subscriptions::use_token(&state, &token, false, now)
        .await
        .unwrap();
    assert!(
        !mail::deliver_one(&state, &smtp, now + time::Duration::minutes(2))
            .await
            .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT cancelled FROM mail_queue WHERE purpose='document'")
            .fetch_one(&state.pool)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn imported_notifications_recheck_original_consent_and_public_visibility() {
    let app = App::new().await;
    let recipient = confirmed_subscriber(&app, "withdrawn@example.test").await;
    let now = OffsetDateTime::now_utc();
    imported_notice(&app, recipient, now).await;
    sqlx::query("UPDATE subscription_consents SET withdrawn_at=$1 WHERE id=$2")
        .bind(now.unix_timestamp())
        .bind(recipient.1)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE subscribers SET unsubscribed_at=$1 WHERE id=$2")
        .bind(now.unix_timestamp())
        .bind(recipient.0)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let renewed = confirmed_subscriber(&app, "withdrawn@example.test").await;
    assert_eq!(recipient.0, renewed.0);
    assert_ne!(recipient.1, renewed.1);
    let hidden = imported_notice(&app, renewed, now).await;
    sqlx::query("UPDATE notices SET status='draft' WHERE id=$1")
        .bind(hidden)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let document: i64 = sqlx::query_scalar("INSERT INTO documents(title,status,created_at) VALUES ('Skrytý dokument','archived','now') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO publication_outbox(document_id,subscriber_id,consent_id,created_at) VALUES ($1,$2,$3,$4)")
        .bind(document).bind(renewed.0).bind(renewed.1).bind(now.unix_timestamp()).execute(&app.state.pool).await.unwrap();
    let state = unavailable_smtp(&app).await;
    assert!(
        mail::deliver_one(&state, &mail::transport(&state).unwrap(), now)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn disabled_subscriptions_preserve_import_handoff_and_erasure_removes_it() {
    let app = App::new().await;
    let recipient = confirmed_subscriber(&app, "erase-import@example.test").await;
    let now = OffsetDateTime::now_utc();
    imported_notice(&app, recipient, now).await;
    let mut config = (*app.state.config).clone();
    config.privacy = None;
    let state = Backend::new(app.state.pool.clone(), config);
    assert!(
        !mail::deliver_one(&state, &mail::transport(&state).unwrap(), now)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("DELETE FROM subscribers WHERE id=$1")
        .bind(recipient.0)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        0
    );
}

async fn run_archive_import(app: &App, directory: &std::path::Path) {
    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let output = std::process::Command::new("python3")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("TEST_IMPORT_SCHEMA", schema)
        .arg("-c")
        .arg(r#"
import os, sys
from pathlib import Path
sys.path.insert(0, 'tests')
from test_legacy import navigation_bundle
from legacy_import import import_bundle
from postgres import connect
from psycopg import sql
root = Path(sys.argv[1])
if not (root / 'manifest.json').exists():
    navigation_bundle(root)
with connect(os.environ['TEST_DATABASE_URL']) as conn:
    conn.execute(sql.SQL('SET search_path TO {}').format(sql.Identifier(os.environ['TEST_IMPORT_SCHEMA'])))
    import_bundle(conn, root, publish_content=True, classify_navigation=True, archive_notices=True)
"#)
        .arg(directory)
        .output().unwrap();
    assert!(
        output.status.success(),
        "Import selhal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "vyžaduje Mailpit a Python závislosti, spusťte scripts/test.sh --smtp"]
async fn web_worker_delivers_real_python_archive_import_to_mailpit() {
    let app = App::new().await;
    confirmed_subscriber(&app, "archive-import@example.test").await;
    let directory = tempfile::tempdir().unwrap();
    run_archive_import(&app, directory.path()).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );

    // This is the same worker entrypoint polled by the running web server.
    let mut config = (*app.state.config).clone();
    config.smtp_host = std::env::var("TEST_SMTP_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    config.smtp_port = std::env::var("TEST_SMTP_PORT")
        .unwrap_or_else(|_| "1025".into())
        .parse()
        .unwrap();
    let state = Backend::new(app.state.pool.clone(), config);
    let mut worker = mail::Worker::new(&state).unwrap();
    assert!(
        worker
            .deliver_one(&state, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    assert!(
        worker
            .deliver_one(&state, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    assert!(
        !worker
            .deliver_one(&state, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document' AND sent_at IS NOT NULL AND body IS NULL AND attempts=1").fetch_one(&state.pool).await.unwrap(), 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notice_events")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        0
    );
    assert!(sqlx::query_scalar::<_, bool>("SELECT status='archived' AND published_at IS NULL AND withdrawn_at IS NULL FROM notices").fetch_one(&state.pool).await.unwrap());
    run_archive_import(&app, directory.path()).await;
    assert!(
        !worker
            .deliver_one(&state, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        2
    );
}
