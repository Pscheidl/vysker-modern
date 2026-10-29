#![cfg(feature = "ssr")]
mod common;

use common::App;
use obecni_web::backend::{Backend, auth, mail, subscriptions};
use sqlx::{PgConnection, postgres::PgPoolOptions};
use time::{Duration, OffsetDateTime};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn hour() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    OffsetDateTime::from_unix_timestamp((now / 3600 + 1) * 3600).unwrap()
}

async fn recipient(app: &App, email: &str) -> i64 {
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    subscriptions::use_token(&app.state, &app.confirmation_token(email).await, true, now)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT id FROM subscribers WHERE email=$1")
        .bind(email)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn event(app: &App, title: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO events(title,location,starts_at,ends_at,published,updated_at)
        VALUES ($1,'Obecni sal','2026-11-05T17:30:00Z','2026-11-05T19:00:00Z',TRUE,'2026-10-29T10:00:00Z') RETURNING id")
        .bind(title).fetch_one(&app.state.pool).await.unwrap()
}

async fn document(app: &App) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO documents(title,status,created_at)
        VALUES ('Obecni dokument','published','2026-10-29T10:00:00Z') RETURNING id",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

async fn notice(app: &App) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO notices(title,status,published_on,published_at)
        VALUES ('Uredni oznameni','published','2026-10-29','2026-10-29T10:00:00Z') RETURNING id",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

async fn enqueue(app: &App, target: mail::PublicationTarget, at: OffsetDateTime) {
    let mut tx = obecni_web::db::begin_write(&app.state.pool).await.unwrap();
    mail::enqueue_publication(&mut tx, target, at)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn counts(app: &App) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM publication_outbox),
        (SELECT count(*) FROM mail_queue WHERE purpose='document'),
        (SELECT count(*) FROM mail_publication_items)",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

async fn digest(app: &App) -> (String, String) {
    sqlx::query_as(
        "SELECT subject,body FROM mail_queue WHERE purpose='document' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
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
async fn calendar_events_share_one_hourly_digest_and_cannot_be_announced_twice() {
    let app = App::new().await;
    recipient(&app, "mixed-calendar@example.test").await;
    let cutoff = hour();
    let published_at = cutoff - Duration::seconds(1);
    let event_id = event(&app, "Setkani obcanu").await;
    let document_id = document(&app).await;
    let notice_id = notice(&app).await;
    enqueue(&app, mail::PublicationTarget::Event(event_id), published_at).await;
    enqueue(
        &app,
        mail::PublicationTarget::Notice(notice_id),
        published_at,
    )
    .await;
    enqueue(
        &app,
        mail::PublicationTarget::Document(document_id),
        published_at,
    )
    .await;
    enqueue(&app, mail::PublicationTarget::Event(event_id), published_at).await;
    assert_eq!(counts(&app).await, (3, 0, 0));
    assert!(
        !mail::prepare_publications(&app.state, published_at)
            .await
            .unwrap()
    );
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 1, 3));
    let (subject, body) = digest(&app).await;
    assert_eq!(subject, "Vyskeř: Souhrn novinek (3)");
    assert!(body.contains("Setkani obcanu\nZačátek: 5. 11. 2026 v 18:30"));
    assert!(body.contains("Konec: 5. 11. 2026 v 20:00"));
    assert!(body.contains("Místo: Obecni sal"));
    for route in [
        format!("/kalendar/{event_id}"),
        format!("/dokumenty/{document_id}"),
        format!("/uredni-deska/{notice_id}"),
    ] {
        assert_eq!(body.matches(&route).count(), 1);
    }

    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let delivered = capture.message().await;
    assert!(delivered.contains(&format!("/kalendar/{event_id}")));
    assert!(delivered.contains(&format!("/dokumenty/{document_id}")));
    assert!(delivered.contains(&format!("/uredni-deska/{notice_id}")));
    enqueue(&app, mail::PublicationTarget::Event(event_id), cutoff).await;
    assert!(
        mail::prepare_publications(&app.state, cutoff + Duration::hours(1))
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 1, 3));
    assert!(
        !mail::deliver_one(&state, &smtp, cutoff + Duration::hours(1))
            .await
            .unwrap()
    );
    assert!(capture.messages.try_recv().is_err());
}

#[tokio::test]
async fn calendar_only_preferences_do_not_enable_documents_or_notices() {
    let app = App::new().await;
    let calendar = recipient(&app, "only-calendar@example.test").await;
    let documents = recipient(&app, "only-documents@example.test").await;
    let none = recipient(&app, "no-topics@example.test").await;
    sqlx::query(
        "UPDATE subscribers SET all_notice_categories=FALSE,notice_category_ids='{}',
        uncategorized_notices=FALSE,documents=(id=$1),events=(id=$2)",
    )
    .bind(documents)
    .bind(calendar)
    .execute(&app.state.pool)
    .await
    .unwrap();
    let cutoff = hour();
    let event_id = event(&app, "Pouze kalendar").await;
    let document_id = document(&app).await;
    let notice_id = notice(&app).await;
    let at = cutoff - Duration::seconds(1);
    enqueue(&app, mail::PublicationTarget::Event(event_id), at).await;
    enqueue(&app, mail::PublicationTarget::Document(document_id), at).await;
    enqueue(&app, mail::PublicationTarget::Notice(notice_id), at).await;
    let recipients: Vec<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT subscriber_id,event_id,document_id FROM publication_outbox ORDER BY subscriber_id",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        recipients,
        vec![
            (calendar, Some(event_id), None),
            (documents, None, Some(document_id))
        ]
    );
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
    assert!(
        !mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    let messages: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT subscriber_id,subject,body FROM mail_queue WHERE purpose='document' ORDER BY subscriber_id")
        .fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].0, calendar);
    assert_eq!(messages[0].1, "Vyskeř: Souhrn novinek (1)");
    assert!(!messages[0].2.contains("/dokumenty/"));
    assert_eq!(messages[1].0, documents);
    assert_eq!(messages[1].1, "Vyskeř: Souhrn nových dokumentů (1)");
    assert!(!messages[1].2.contains("/kalendar/"));
    assert!(messages.iter().all(|message| message.0 != none));
}

#[tokio::test]
async fn calendar_labels_preserve_all_day_unknown_times_ranges_and_recurrence() {
    let app = App::new().await;
    recipient(&app, "calendar-times@example.test").await;
    let cutoff = hour();
    let all_day = event(&app, "Celodenni akce").await;
    let unknown = event(&app, "Nezname casy").await;
    let range = event(&app, "Vicedenni opakovana akce").await;
    sqlx::query(
        "UPDATE events SET all_day=TRUE,starts_at='2026-11-05T00:00:00+01:00',
        ends_at='2026-11-05T23:59:59+01:00' WHERE id=$1",
    )
    .bind(all_day)
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE events SET starts_at='2026-11-05T00:00:00+01:00',
        ends_at='2026-11-05T23:59:59+01:00',start_time_known=FALSE,end_time_known=FALSE,end_date_known=FALSE WHERE id=$1")
        .bind(unknown).execute(&app.state.pool).await.unwrap();
    sqlx::query("UPDATE events SET all_day=TRUE,starts_at='2026-11-05T00:00:00+01:00',
        ends_at='2026-11-06T23:59:59+01:00',repeat_every_14_days=TRUE,repeat_until='2026-12-31' WHERE id=$1")
        .bind(range).execute(&app.state.pool).await.unwrap();
    for id in [all_day, unknown, range] {
        enqueue(
            &app,
            mail::PublicationTarget::Event(id),
            cutoff - Duration::seconds(1),
        )
        .await;
    }
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    let (_, body) = digest(&app).await;
    assert!(body.contains("Celodenni akce\nTermín: 5. 11. 2026 (celý den)"));
    assert!(body.contains("Nezname casy\nZačátek: 5. 11. 2026 (původní web čas neuváděl)\nKonec: Původní web datum konce neuváděl"));
    assert!(body.contains(
        "Vicedenni opakovana akce\nZačátek: 5. 11. 2026 (celý den)\nKonec: 6. 11. 2026 (celý den)"
    ));
    assert!(body.contains("Opakuje se každých 14 dní, nejpozději do 31. 12. 2026 včetně"));
    assert!(!body.contains("00:00"));
    assert!(!body.contains("23:59"));
}

#[tokio::test]
async fn changed_events_are_excluded_before_materialization() {
    let app = App::new().await;
    recipient(&app, "calendar-before-digest@example.test").await;
    let cutoff = hour();
    let keep = event(&app, "Zachovana akce").await;
    let hide = event(&app, "Skryta akce").await;
    let cancel = event(&app, "Zrusena akce").await;
    let delete = event(&app, "Smazana akce").await;
    for id in [keep, hide, cancel, delete] {
        enqueue(
            &app,
            mail::PublicationTarget::Event(id),
            cutoff - Duration::seconds(1),
        )
        .await;
    }
    sqlx::query("UPDATE events SET published=FALSE WHERE id=$1")
        .bind(hide)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE events SET cancelled=TRUE WHERE id=$1")
        .bind(cancel)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM events WHERE id=$1")
        .bind(delete)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    assert_eq!(counts(&app).await, (0, 1, 1));
    let (_, body) = digest(&app).await;
    assert!(body.contains(&format!("/kalendar/{keep}")));
    for id in [hide, cancel, delete] {
        assert!(!body.contains(&format!("/kalendar/{id}")));
        enqueue(&app, mail::PublicationTarget::Event(id), cutoff).await;
    }
    assert_eq!(counts(&app).await, (0, 1, 1));
}

#[tokio::test]
async fn event_opt_out_before_materialization_preserves_document_delivery() {
    let app = App::new().await;
    let recipient_id = recipient(&app, "calendar-preparation-opt-out@example.test").await;
    let cutoff = hour();
    let event_id = event(&app, "Odmitnuta akce").await;
    let document_id = document(&app).await;
    enqueue(
        &app,
        mail::PublicationTarget::Event(event_id),
        cutoff - Duration::seconds(1),
    )
    .await;
    enqueue(
        &app,
        mail::PublicationTarget::Document(document_id),
        cutoff - Duration::seconds(1),
    )
    .await;
    sqlx::query("UPDATE subscribers SET events=FALSE WHERE id=$1")
        .bind(recipient_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    let (subject, body) = digest(&app).await;
    assert_eq!(subject, "Vyskeř: Souhrn nových dokumentů (1)");
    assert!(body.contains(&format!("/dokumenty/{document_id}")));
    assert!(!body.contains("/kalendar/"));
    assert_eq!(counts(&app).await, (0, 1, 1));
}

#[tokio::test]
async fn smtp_retry_keeps_identity_and_permanently_removes_ineligible_events() {
    let app = App::new().await;
    recipient(&app, "calendar-retry@example.test").await;
    let cutoff = hour();
    let keep = event(&app, "Zachovana akce").await;
    let hide = event(&app, "Skryta akce").await;
    let cancel = event(&app, "Zrusena akce").await;
    let delete = event(&app, "Smazana akce").await;
    let document_id = document(&app).await;
    for id in [keep, hide, cancel, delete] {
        enqueue(
            &app,
            mail::PublicationTarget::Event(id),
            cutoff - Duration::seconds(1),
        )
        .await;
    }
    enqueue(
        &app,
        mail::PublicationTarget::Document(document_id),
        cutoff - Duration::seconds(1),
    )
    .await;
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE events SET published=FALSE WHERE id=$1")
        .bind(hide)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE events SET cancelled=TRUE WHERE id=$1")
        .bind(cancel)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM events WHERE id=$1")
        .bind(delete)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let mut capture = Capture::new(true).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let first = capture.message().await;
    assert!(first.contains(&format!("/kalendar/{keep}")));
    assert!(first.contains(&format!("/dokumenty/{document_id}")));
    for id in [hide, cancel, delete] {
        assert!(!first.contains(&format!("/kalendar/{id}")));
    }
    let (attempts, sent_at, retry_at): (i64, Option<i64>, i64) = sqlx::query_as(
        "SELECT attempts,sent_at,next_attempt_at FROM mail_queue WHERE purpose='document'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!((attempts, sent_at), (1, None));
    sqlx::query("UPDATE events SET published=TRUE,cancelled=FALSE WHERE id=ANY($1)")
        .bind(vec![hide, cancel])
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(
        mail::deliver_one(
            &state,
            &smtp,
            OffsetDateTime::from_unix_timestamp(retry_at).unwrap()
        )
        .await
        .unwrap()
    );
    let second = capture.message().await;
    let message_id = |message: &str| {
        message
            .lines()
            .find(|line| line.starts_with("Message-ID:"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(message_id(&first), message_id(&second));
    assert!(second.contains(&format!("/kalendar/{keep}")));
    for id in [hide, cancel, delete] {
        assert!(!second.contains(&format!("/kalendar/{id}")));
    }
    let suppressed: Vec<i64> = sqlx::query_scalar(
        "SELECT event_id FROM mail_publication_items WHERE suppressed ORDER BY event_id",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert_eq!(suppressed, vec![hide, cancel]);
}

#[tokio::test]
async fn calendar_opt_out_before_smtp_preserves_other_topics() {
    let app = App::new().await;
    let recipient_id = recipient(&app, "calendar-send-opt-out@example.test").await;
    let cutoff = hour();
    let event_id = event(&app, "Odmitnuta akce").await;
    let document_id = document(&app).await;
    enqueue(
        &app,
        mail::PublicationTarget::Event(event_id),
        cutoff - Duration::seconds(1),
    )
    .await;
    enqueue(
        &app,
        mail::PublicationTarget::Document(document_id),
        cutoff - Duration::seconds(1),
    )
    .await;
    assert!(
        mail::prepare_publications(&app.state, cutoff)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE subscribers SET events=FALSE WHERE id=$1")
        .bind(recipient_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
    let sent = capture.message().await;
    assert!(sent.contains(&format!("/dokumenty/{document_id}")));
    assert!(!sent.contains("/kalendar/"));
    let subject: String =
        sqlx::query_scalar("SELECT subject FROM mail_queue WHERE purpose='document'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(subject, "Vyskeř: Souhrn nových dokumentů (1)");
}

#[tokio::test]
async fn event_only_digests_cancel_without_smtp_after_opt_out_or_removal() {
    let app = App::new().await;
    let recipient_id = recipient(&app, "calendar-cancelled-mail@example.test").await;
    let mut capture = Capture::new(false).await;
    let state = capture.backend(&app);
    let smtp = mail::transport(&state).unwrap();
    let start = hour();
    for (index, change) in ["opt_out", "unpublish", "cancel", "delete"]
        .into_iter()
        .enumerate()
    {
        let cutoff = start + Duration::hours(index as i64);
        sqlx::query("UPDATE subscribers SET events=TRUE WHERE id=$1")
            .bind(recipient_id)
            .execute(&state.pool)
            .await
            .unwrap();
        let event_id = event(&app, change).await;
        enqueue(
            &app,
            mail::PublicationTarget::Event(event_id),
            cutoff - Duration::seconds(1),
        )
        .await;
        assert!(mail::prepare_publications(&state, cutoff).await.unwrap());
        match change {
            "opt_out" => sqlx::query("UPDATE subscribers SET events=FALSE WHERE id=$1")
                .bind(recipient_id)
                .execute(&state.pool)
                .await
                .unwrap(),
            "unpublish" => sqlx::query("UPDATE events SET published=FALSE WHERE id=$1")
                .bind(event_id)
                .execute(&state.pool)
                .await
                .unwrap(),
            "cancel" => sqlx::query("UPDATE events SET cancelled=TRUE WHERE id=$1")
                .bind(event_id)
                .execute(&state.pool)
                .await
                .unwrap(),
            "delete" => sqlx::query("DELETE FROM events WHERE id=$1")
                .bind(event_id)
                .execute(&state.pool)
                .await
                .unwrap(),
            _ => unreachable!(),
        };
        assert!(mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
        assert!(!mail::deliver_one(&state, &smtp, cutoff).await.unwrap());
        assert!(
            capture.messages.try_recv().is_err(),
            "Cancelled event mail must never reach SMTP DATA"
        );
    }
    let cancelled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_queue
        WHERE purpose='document' AND cancelled AND body IS NULL AND sent_at IS NULL",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(cancelled, 4);
}

async fn snapshot(conn: &mut PgConnection, table: &'static str, added_column: &str) -> Vec<String> {
    sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT (to_jsonb(t)-")
        .push_bind(added_column)
        .push("::text)::text FROM ")
        .push(table)
        .push(" t ORDER BY 1")
        .build_query_scalar()
        .fetch_all(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn calendar_upgrade_enables_current_subscribers_without_rewriting_history_or_backfilling_mail()
 {
    let url =
        std::env::var("TEST_DATABASE_URL").expect("Run scripts/test.sh or set TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("calendar_mail_upgrade_{}", &auth::token()[..24]);
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('search_path',$1,true)")
        .bind(&schema)
        .execute(&mut *tx)
        .await
        .unwrap();
    let migrations = sqlx::migrate!();
    for migration in migrations
        .iter()
        .filter(|migration| migration.version <= 25)
    {
        sqlx::raw_sql(migration.sql.clone())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    sqlx::raw_sql(
        "INSERT INTO consent_notices(fingerprint,version,consent_text,privacy_json)
            VALUES ('original','2026-10','Original confirmation text','{}');
         INSERT INTO subscribers(id,email,verified_at,retention_started_at,unsubscribed_at,
             all_notice_categories,notice_category_ids,uncategorized_notices,documents)
            VALUES (41,'confirmed@example.test',1100,1000,NULL,FALSE,ARRAY[1,3],FALSE,TRUE),
                   (42,'pending@example.test',NULL,1000,NULL,TRUE,'{}',TRUE,FALSE),
                   (43,'withdrawn@example.test',1100,1000,1200,FALSE,'{}',FALSE,TRUE);
         INSERT INTO subscription_consents(id,subscriber_id,notice_fingerprint,requested_at,confirmed_at,withdrawn_at,
             all_notice_categories,notice_category_ids,uncategorized_notices,documents)
            VALUES (51,41,'original',1000,1100,NULL,TRUE,'{}',TRUE,TRUE),
                   (52,42,'original',1000,NULL,NULL,FALSE,'{}',FALSE,TRUE),
                   (53,43,'original',1000,1100,1200,TRUE,'{}',TRUE,FALSE);
         INSERT INTO documents(id,title,status,created_at)
            VALUES (101,'Pending publication','published','2026-10-29T10:00:00Z'),
                   (102,'Prepared digest item','published','2026-10-29T10:00:00Z');
         INSERT INTO publication_outbox(id,document_id,subscriber_id,consent_id,created_at)
            VALUES (61,101,41,51,1500);
         INSERT INTO mail_queue(id,subscriber_id,consent_id,purpose,deduplication_key,subject,body,
             attempts,next_attempt_at,locked_until,lock_token,created_at,digest_cutoff)
            VALUES (71,41,51,'document','digest:41:51:1000','Original subject','Original body',
                    3,1700,2000,'original-lease',1500,1000);
         INSERT INTO mail_publication_items(id,mail_id,subscriber_id,document_id)
            VALUES (81,71,41,102);
         INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id)
            VALUES ('unchanged-token-hash',41,'unsubscribe',999999,51);
         INSERT INTO events(id,title,starts_at,ends_at,published,cancelled,updated_at)
            VALUES (91,'Already public','2026-11-05T10:00:00Z','2026-11-05T11:00:00Z',TRUE,FALSE,'2026-10-29T10:00:00Z'),
                   (92,'Old cancelled','2020-11-05T10:00:00Z','2020-11-05T11:00:00Z',TRUE,TRUE,'2026-10-29T10:00:00Z'),
                   (93,'Unpublished draft','2026-11-05T10:00:00Z','2026-11-05T11:00:00Z',FALSE,FALSE,'2026-10-29T10:00:00Z'),
                   (94,'Formerly public','2026-11-05T10:00:00Z','2026-11-05T11:00:00Z',TRUE,FALSE,'2026-10-29T10:00:00Z'),
                   (95,'Deleted import','2026-11-05T10:00:00Z','2026-11-05T11:00:00Z',TRUE,FALSE,'2026-10-29T10:00:00Z');
         UPDATE events SET published=FALSE WHERE id=94;
         INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,event_id,destination)
            SELECT 'event-' || id,'https://legacy.example.test/event/' || id,'original-fingerprint',
                '2026-10-28T10:00:00Z','2026-10-29T10:00:00Z','{}',id,'/kalendar/' || id FROM events;
         DELETE FROM events WHERE id=95;"
    ).execute(&mut *tx).await.unwrap();
    let tables = [
        ("subscribers", "events"),
        ("subscription_consents", "events"),
        ("consent_notices", ""),
        ("subscription_tokens", ""),
        ("mail_queue", ""),
        ("publication_outbox", "event_id"),
        ("mail_publication_items", "event_id"),
        ("events", "email_announced_at"),
        ("calendar_withdrawals", ""),
        ("legacy_sources", ""),
    ];
    let mut before = Vec::new();
    for (table, column) in tables {
        before.push(snapshot(&mut tx, table, column).await);
    }
    let upgrade = migrations
        .iter()
        .find(|migration| migration.version == 26)
        .unwrap();
    sqlx::raw_sql(upgrade.sql.clone())
        .execute(&mut *tx)
        .await
        .unwrap();
    for ((table, column), expected) in tables.into_iter().zip(before) {
        assert_eq!(
            snapshot(&mut tx, table, column).await,
            expected,
            "Changed existing {table}"
        );
    }
    let defaults: Vec<bool> = sqlx::query_scalar("SELECT events FROM subscribers ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(defaults, vec![true, true, true]);
    let historical_consent: Vec<Option<bool>> =
        sqlx::query_scalar("SELECT events FROM subscription_consents ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(historical_consent, vec![None, None, None]);
    let announcement_boundary: Vec<bool> =
        sqlx::query_scalar("SELECT email_announced_at IS NOT NULL FROM events ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(announcement_boundary, vec![true, true, false, true]);
    let import_announcements: Vec<(String, bool)> = sqlx::query_as(
        "SELECT a.source_key,coalesce(a.announced_at=e.email_announced_at,e.id IS NULL AND a.announced_at>0)
         FROM legacy_event_announcements a
         JOIN legacy_sources s ON s.source_key=a.source_key
         LEFT JOIN events e ON e.id=s.event_id ORDER BY a.source_key",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        import_announcements,
        vec![
            ("event-91".into(), true),
            ("event-92".into(), true),
            ("event-94".into(), true),
            ("event-95".into(), true),
        ]
    );
    let event_requests: i64 =
        sqlx::query_scalar("SELECT count(*) FROM publication_outbox WHERE event_id IS NOT NULL")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(event_requests, 0);
    let matching: (bool, bool, bool, bool, bool) = sqlx::query_as(
        "SELECT subscription_matches_publication(41,NULL,101),
            subscription_matches_publication(41,NULL,NULL,91),
            subscription_matches_publication(41,NULL,NULL,92),
            subscription_matches_publication(41,NULL,NULL,93),
            subscription_matches_publication(41,NULL,101,91)",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(matching, (true, true, false, false, false));

    sqlx::query("SAVEPOINT immutable_history")
        .execute(&mut *tx)
        .await
        .unwrap();
    let change = sqlx::query("UPDATE subscription_consents SET events=TRUE WHERE id=51")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(change.to_string().contains("consent evidence is immutable"));
    sqlx::query("ROLLBACK TO SAVEPOINT immutable_history")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    pool.close().await;
}
