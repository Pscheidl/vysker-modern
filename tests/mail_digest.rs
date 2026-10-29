#![cfg(feature = "ssr")]
mod common;

use common::App;
use obecni_web::backend::{Backend, auth, mail, subscriptions};
use time::{Duration, OffsetDateTime};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn hour() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    OffsetDateTime::from_unix_timestamp((now / 3600 + 1) * 3600).unwrap()
}

async fn recipient(app: &App, email: &str) -> (i64, i64) {
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    subscriptions::use_token(&app.state, &app.confirmation_token(email).await, true, now)
        .await
        .unwrap();
    sqlx::query_as("SELECT s.id,c.id FROM subscribers s JOIN subscription_consents c ON c.subscriber_id=s.id WHERE s.email=$1 AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL")
        .bind(email).fetch_one(&app.state.pool).await.unwrap()
}

async fn document(app: &App, recipient: (i64, i64), title: &str, at: OffsetDateTime) -> i64 {
    let id = sqlx::query_scalar("INSERT INTO documents(title,status,created_at) VALUES ($1,'published','2026-01-01T00:00:00Z') RETURNING id")
        .bind(title).fetch_one(&app.state.pool).await.unwrap();
    enqueue(app, recipient, id, at).await;
    id
}

async fn enqueue(app: &App, recipient: (i64, i64), document: i64, at: OffsetDateTime) {
    sqlx::query("INSERT INTO publication_outbox(document_id,subscriber_id,consent_id,created_at) VALUES ($1,$2,$3,$4)")
        .bind(document).bind(recipient.0).bind(recipient.1).bind(at.unix_timestamp())
        .execute(&app.state.pool).await.unwrap();
}

async fn counts(app: &App) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM publication_outbox),(SELECT count(*) FROM mail_queue WHERE purpose='document'),(SELECT count(*) FROM mail_publication_items)")
        .fetch_one(&app.state.pool).await.unwrap()
}

struct Capture {
    port: u16,
    messages: tokio::sync::mpsc::UnboundedReceiver<String>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Capture {
    async fn new(reject_first: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (send, messages) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut reject_next = reject_first;
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let (read, mut write) = socket.into_split();
                let mut read = BufReader::new(read);
                write.write_all(b"220 local.test ESMTP\r\n").await.unwrap();
                let mut line = String::new();
                loop {
                    line.clear();
                    if read.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    let reply: &[u8] = if line.starts_with("EHLO") || line.starts_with("HELO") {
                        b"250 local.test\r\n"
                    } else if line.starts_with("DATA") {
                        write.write_all(b"354 Send message\r\n").await.unwrap();
                        let mut message = String::new();
                        loop {
                            line.clear();
                            assert!(read.read_line(&mut line).await.unwrap() > 0);
                            if line == ".\r\n" {
                                break;
                            }
                            message.push_str(&line);
                        }
                        send.send(message).unwrap();
                        if reject_next {
                            reject_next = false;
                            b"451 Temporary failure\r\n"
                        } else {
                            b"250 Accepted\r\n"
                        }
                    } else if line.starts_with("QUIT") {
                        write.write_all(b"221 Bye\r\n").await.unwrap();
                        break;
                    } else {
                        b"250 OK\r\n"
                    };
                    write.write_all(reply).await.unwrap();
                }
            }
        });
        Self {
            port,
            messages,
            task,
        }
    }
    fn backend(&self, app: &App) -> Backend {
        let mut config = (*app.state.config).clone();
        config.smtp_port = self.port;
        Backend::new(app.state.pool.clone(), config)
    }
    async fn message(&mut self) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(5), self.messages.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

#[tokio::test]
async fn hour_boundary_groups_equal_timestamps_and_recovers_late_commits() {
    let app = App::new().await;
    let recipient = recipient(&app, "boundaries@example.test").await;
    let cutoff = hour();
    let a = document(&app, recipient, "First", cutoff - Duration::seconds(1)).await;
    let b = document(&app, recipient, "Second", cutoff - Duration::seconds(1)).await;
    let c = document(&app, recipient, "Boundary", cutoff).await;
    assert!(
        !mail::prepare_publications(&app.state, cutoff - Duration::seconds(1))
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (3, 0, 0));
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (1, 1, 2));
    let body: String = sqlx::query_scalar("SELECT body FROM mail_queue WHERE purpose='document'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(body.contains(&format!("/dokumenty/{a}")));
    assert!(body.contains(&format!("/dokumenty/{b}")));
    assert!(!body.contains(&format!("/dokumenty/{c}\n")));

    // A transaction can commit later with an earlier publication timestamp.
    let late = document(
        &app,
        recipient,
        "Late commit",
        cutoff - Duration::seconds(2),
    )
    .await;
    assert!(
        !mail::prepare_publications(&app.state, cutoff + Duration::seconds(1))
            .await
            .unwrap()
    );
    assert!(
        mail::prepare_publications(&app.state, cutoff + Duration::hours(1))
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 2, 4));
    let items: Vec<i64> =
        sqlx::query_scalar("SELECT document_id FROM mail_publication_items ORDER BY document_id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(items, vec![a, b, c, late]);
    assert!(
        !mail::prepare_publications(&app.state, cutoff + Duration::hours(2))
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 2, 4));
}

#[tokio::test]
async fn concurrent_preparation_keeps_more_than_one_hundred_items_in_one_digest() {
    let app = App::new().await;
    let recipient = recipient(&app, "many@example.test").await;
    let cutoff = hour();
    for n in 0..125 {
        document(
            &app,
            recipient,
            &format!("Document {n:03}"),
            cutoff - Duration::seconds(1),
        )
        .await;
    }
    let (first, second) = tokio::join!(
        mail::prepare_publications(&app.state, cutoff),
        mail::prepare_publications(&app.state, cutoff)
    );
    assert_ne!(first.unwrap(), second.unwrap());
    assert_eq!(counts(&app).await, (0, 1, 125));
    let (body, subject): (String, String) =
        sqlx::query_as("SELECT body,subject FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(body.contains("Document 000"));
    assert!(body.contains("Document 124"));
    assert_eq!(body.matches("/dokumenty/").count(), 125);
    assert_eq!(subject, "Vyskeř: Souhrn nových dokumentů (125)");
}

#[tokio::test]
async fn retry_preserves_message_identity_and_permanently_excludes_hidden_items() {
    let app = App::new().await;
    let recipient = recipient(&app, "retry-digest@example.test").await;
    let cutoff = hour();
    let keep = document(
        &app,
        recipient,
        "Keep document",
        cutoff - Duration::seconds(1),
    )
    .await;
    let hide = document(
        &app,
        recipient,
        "Hide document",
        cutoff - Duration::seconds(1),
    )
    .await;
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE documents SET status='draft' WHERE id=$1")
        .bind(hide)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let mut capture = Capture::new(true).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let first = capture.message().await;
    assert!(first.contains(&format!("/dokumenty/{keep}")));
    assert!(!first.contains(&format!("/dokumenty/{hide}")));
    let retry: (i64, Option<i64>, i64, String) = sqlx::query_as(
        "SELECT attempts,sent_at,next_attempt_at,body FROM mail_queue WHERE purpose='document'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(retry.0, 1);
    assert_eq!(retry.1, None);
    assert!(retry.2 > cutoff.unix_timestamp());
    assert!(!retry.3.contains("Hide document"));
    sqlx::query("UPDATE documents SET status='published' WHERE id=$1")
        .bind(hide)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert!(
        mail::deliver_one(
            &state,
            &smtp,
            OffsetDateTime::from_unix_timestamp(retry.2).unwrap()
        )
        .await
        .unwrap()
    );
    let second = capture.message().await;
    assert!(!second.contains(&format!("/dokumenty/{hide}")));
    let message_id = |value: &str| {
        value
            .lines()
            .find(|line| line.starts_with("Message-ID:"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(message_id(&first), message_id(&second));
    let completed: (i64, bool, bool) = sqlx::query_as(
        "SELECT attempts,sent_at IS NOT NULL,body IS NULL FROM mail_queue WHERE purpose='document'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(completed, (2, true, true));
    assert_eq!(counts(&app).await, (0, 1, 2));
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT suppressed FROM mail_publication_items WHERE document_id=$1"
        )
        .bind(hide)
        .fetch_one(&state.pool)
        .await
        .unwrap()
    );
    // Repeated import handoff also cannot revive a suppressed publication.
    enqueue(&app, recipient, hide, cutoff).await;
    assert!(
        mail::prepare_publications(&state, cutoff + Duration::hours(1))
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 1, 2));
}

#[tokio::test]
async fn one_recipient_can_drop_notices_and_still_receive_documents_in_the_same_digest() {
    let app = App::new().await;
    let recipient = recipient(&app, "mixed@example.test").await;
    let cutoff = hour();
    let doc = document(
        &app,
        recipient,
        "Keep document",
        cutoff - Duration::seconds(1),
    )
    .await;
    let notice: i64 = sqlx::query_scalar("INSERT INTO notices(title,status,review_json) VALUES ('Drop notice','archived','{}') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO publication_outbox(notice_id,subscriber_id,consent_id,created_at) VALUES ($1,$2,$3,$4)")
        .bind(notice).bind(recipient.0).bind(recipient.1).bind(cutoff.unix_timestamp()-1)
        .execute(&app.state.pool).await.unwrap();
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE subscribers SET all_notice_categories=FALSE,uncategorized_notices=FALSE,notice_category_ids='{}',documents=TRUE,events=FALSE WHERE id=$1")
        .bind(recipient.0).execute(&app.state.pool).await.unwrap();
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    assert!(
        mail::deliver_one(&state, &mail::transport(&state).unwrap(), cutoff)
            .await
            .unwrap()
    );
    let sent = capture.message().await;
    assert!(sent.contains(&format!("/dokumenty/{doc}")));
    assert!(!sent.contains("/uredni-deska/"));
    assert_eq!(counts(&app).await, (0, 1, 2));
}

#[tokio::test]
async fn legacy_single_messages_finish_while_unmaterialized_work_becomes_a_digest() {
    let app = App::new().await;
    let recipient = recipient(&app, "migration@example.test").await;
    let cutoff = hour();
    let old = document(
        &app,
        recipient,
        "Old individual",
        cutoff - Duration::seconds(1),
    )
    .await;
    let new = document(
        &app,
        recipient,
        "New summary",
        cutoff - Duration::seconds(1),
    )
    .await;
    let secret = auth::token();
    sqlx::query("INSERT INTO subscription_tokens(hash,subscriber_id,consent_id,purpose,expires_at) VALUES ($1,$2,$3,'unsubscribe',$4)")
        .bind(auth::hash(&secret)).bind(recipient.0).bind(recipient.1).bind(i64::MAX)
        .execute(&app.state.pool).await.unwrap();
    let body = format!(
        "Old individual\nhttp://localhost:3000/dokumenty/{old}\nhttp://localhost:3000/odber/odhlasit?token={secret}"
    );
    sqlx::query("INSERT INTO mail_queue(subscriber_id,consent_id,purpose,deduplication_key,subject,body,next_attempt_at,created_at) VALUES ($1,$2,'document',$3,'Old individual',$4,$5,$5)")
        .bind(recipient.0).bind(recipient.1).bind(format!("document:{old}:{}",recipient.0))
        .bind(body).bind(cutoff.unix_timestamp()-1).execute(&app.state.pool).await.unwrap();
    // The duplicate pending handoff must not put the old item into the new digest.
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 2, 1));
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let first = capture.message().await;
    assert!(first.contains(&format!("/dokumenty/{old}")));
    assert!(!first.contains(&format!("/dokumenty/{new}")));
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let second = capture.message().await;
    assert!(second.contains(&format!("/dokumenty/{new}")));
    assert!(!second.contains(&format!("/dokumenty/{old}")));
    assert!(!mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
}

#[tokio::test]
async fn verification_does_not_wait_for_the_next_hourly_digest() {
    let app = App::new().await;
    let recipient = recipient(&app, "waiting@example.test").await;
    let cutoff = hour();
    let now = cutoff - Duration::seconds(5);
    document(&app, recipient, "Next hour", now).await;
    common::request_subscription(&app.state, "confirm-now@example.test", now)
        .await
        .unwrap();
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    assert!(
        mail::deliver_one(&state, &mail::transport(&state).unwrap(), now)
            .await
            .unwrap()
    );
    let sent = capture.message().await;
    assert!(sent.contains("confirm-now@example.test"));
    assert!(!sent.contains("/dokumenty/"));
    assert_eq!(counts(&app).await, (1, 0, 0));
}

#[tokio::test]
async fn recipients_receive_separate_messages_with_only_their_snapshotted_items() {
    let app = App::new().await;
    let all = recipient(&app, "all-digest@example.test").await;
    let selected = recipient(&app, "selected-digest@example.test").await;
    let cutoff = hour();
    let chosen = document(&app, all, "Chosen document", cutoff - Duration::seconds(1)).await;
    let other = document(&app, all, "Other document", cutoff - Duration::seconds(1)).await;
    enqueue(&app, selected, chosen, cutoff - Duration::seconds(1)).await;
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let first = capture.message().await;
    assert!(first.contains("all-digest@example.test"));
    assert!(!first.contains("selected-digest@example.test"));
    assert!(first.contains(&format!("/dokumenty/{chosen}")));
    assert!(first.contains(&format!("/dokumenty/{other}")));
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let second = capture.message().await;
    assert!(second.contains("selected-digest@example.test"));
    assert!(!second.contains("all-digest@example.test"));
    assert!(second.contains(&format!("/dokumenty/{chosen}")));
    assert!(!second.contains(&format!("/dokumenty/{other}")));
    assert!(!mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    assert_eq!(counts(&app).await, (0, 2, 3));
}

#[tokio::test]
async fn unsubscribe_and_deleting_all_items_cancel_materialized_digests_without_smtp() {
    let app = App::new().await;
    let unsubscribed = recipient(&app, "unsubscribe-digest@example.test").await;
    let deleted = recipient(&app, "deleted-digest@example.test").await;
    let cutoff = hour();
    document(
        &app,
        unsubscribed,
        "Unsubscribed document",
        cutoff - Duration::seconds(1),
    )
    .await;
    let removed = document(
        &app,
        deleted,
        "Deleted document",
        cutoff - Duration::seconds(1),
    )
    .await;
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 2, 2));
    let body: String = sqlx::query_scalar(
        "SELECT body FROM mail_queue WHERE subscriber_id=$1 AND purpose='document'",
    )
    .bind(unsubscribed.0)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    subscriptions::use_token(&app.state, &common::token_from_body(&body), false, cutoff)
        .await
        .unwrap();
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(removed)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(counts(&app).await, (0, 2, 1));
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    assert!(!mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    assert!(
        capture.messages.try_recv().is_err(),
        "No cancelled digest may reach SMTP DATA"
    );
    let cancelled: Vec<(i64, bool, bool, bool, i64)> = sqlx::query_as("SELECT subscriber_id,cancelled,body IS NULL,sent_at IS NULL,attempts FROM mail_queue WHERE purpose='document' ORDER BY subscriber_id")
        .fetch_all(&state.pool).await.unwrap();
    assert_eq!(
        cancelled,
        vec![
            (unsubscribed.0, true, true, true, 0),
            (deleted.0, true, true, true, 1)
        ]
    );
}

#[tokio::test]
async fn administrator_recovery_and_each_report_send_immediately_outside_document_digests() {
    let app = App::new().await;
    let subscriber = recipient(&app, "publication-waiting@example.test").await;
    let response = app
        .call(
            "POST",
            "/api/v1/admin/password-recovery",
            Some(serde_json::json!({"email":"admin@vysker.test"})),
            false,
        )
        .await;
    assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
    let recovery_created: i64 = sqlx::query_scalar("SELECT created_at FROM recovery_mail")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let now = OffsetDateTime::from_unix_timestamp(recovery_created + 1).unwrap();
    document(&app, subscriber, "Hourly publication", now).await;
    for number in 1..=2 {
        let report_id: i64 = sqlx::query_scalar("INSERT INTO issue_reports(description,address,request_ip,created_at) VALUES ($1,'Vysker 1','127.0.0.1','2026-10-26T10:30:00Z') RETURNING id")
            .bind(format!("Report {number}"))
            .fetch_one(&app.state.pool).await.unwrap();
        sqlx::query("INSERT INTO report_mail_queue(report_id,recipient,subject,body,next_attempt_at,created_at) VALUES ($1,'admin@vysker.test',$2,$3,$4,$4)")
            .bind(report_id).bind(format!("Report {number}"))
            .bind(format!("Immediate report {number}"))
            .bind(now.unix_timestamp()).execute(&app.state.pool).await.unwrap();
    }
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, now).await.unwrap());
    let recovery = capture.message().await;
    assert!(recovery.contains("admin@vysker.test"));
    assert!(recovery.contains("/admin/obnova"));
    assert!(!recovery.contains("/dokumenty/"));
    for number in 1..=2 {
        assert!(mail::deliver_one(&state, &smtp, now).await.unwrap());
        let report = capture.message().await;
        assert!(report.contains("admin@vysker.test"));
        assert!(report.contains(&format!("Subject: Report {number}")));
        assert!(report.contains(&format!("Immediate report {number}")));
        assert!(!report.contains("/dokumenty/"));
        assert!(!report.contains("/admin/obnova"));
    }
    assert!(!mail::deliver_one(&state, &smtp, now).await.unwrap());
    assert!(capture.messages.try_recv().is_err());
    assert_eq!(counts(&app).await, (1, 0, 0));
    let sent: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM recovery_mail WHERE sent_at IS NOT NULL AND body IS NULL AND attempts=1),(SELECT count(*) FROM report_mail_queue WHERE sent_at IS NOT NULL AND body IS NULL AND attempts=1)")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(sent, (1, 2));
}
