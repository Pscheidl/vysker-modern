#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::backend::{
    attachment_index::{EXTRACTOR_VERSION, RunOutcome, WorkerOptions, run_once},
    mail, subscriptions,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

fn extractor(succeeds: bool) -> (tempfile::TempDir, WorkerOptions) {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("extract.py");
    // Exercise the production worker's OCR result path without requiring native OCR tools.
    let result = if succeeds {
        json!({"status":"ready","method":"ocr","warnings":[],"pages":[
            {"page":1,"label":"Strana 1","text":"Zastupitelstvo schválilo pronájem hospody."}
        ]})
    } else {
        json!({"status":"failed","error":"Rozpoznávání textu se nezdařilo."})
    };
    std::fs::write(
        &script,
        format!(
            "import sys\nprint({:?})\nsys.exit({})\n",
            result.to_string(),
            if succeeds { 0 } else { 1 }
        ),
    )
    .unwrap();
    let options = WorkerOptions {
        extractor: script,
        timeout_seconds: 5,
        ..Default::default()
    };
    (directory, options)
}

/// Capture only the source, publication history and this subscriber's notification state.
/// A fresh status timestamp in the derived index is deliberately outside this snapshot.
async fn publication_state(pool: &PgPool, document: i64, subscriber: i64) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'source',jsonb_build_object('content_version',d.content_version,
                'fingerprint',document_fingerprint(d.id),'content_sha256',d.content_sha256,
                'published_at',d.published_at,'lock_state',d.lock_state,'public_access',d.public_access),
            'events',coalesce((SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM document_events e WHERE e.document_id=d.id),'[]'::jsonb),
            'legacy_events',coalesce((SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM notice_events e WHERE e.notice_id=d.id),'[]'::jsonb),
            'postings',coalesce((SELECT jsonb_agg(to_jsonb(p) ORDER BY p.id) FROM document_postings p WHERE p.document_id=d.id),'[]'::jsonb),
            'revisions',coalesce((SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM document_import_revisions r WHERE r.document_id=d.id OR r.accepted_document_id=d.id),'[]'::jsonb),
            'announcements',coalesce((SELECT jsonb_agg(to_jsonb(a)) FROM document_announcements a WHERE a.document_id=d.id),'[]'::jsonb),
            'outbox',coalesce((SELECT jsonb_agg(to_jsonb(o) ORDER BY o.id) FROM publication_outbox o WHERE o.subscriber_id=$2),'[]'::jsonb),
            'mail_queue',coalesce((SELECT jsonb_agg(to_jsonb(m) ORDER BY m.id) FROM mail_queue m WHERE m.subscriber_id=$2),'[]'::jsonb),
            'digest_items',coalesce((SELECT jsonb_agg(to_jsonb(i) ORDER BY i.mail_id,i.document_id,i.notice_id,i.event_id) FROM mail_publication_items i WHERE i.subscriber_id=$2),'[]'::jsonb),
            'next_digest_at',(SELECT next_digest_at FROM subscribers WHERE id=$2)
        ) FROM documents d WHERE d.id=$1",
    )
    .bind(document)
    .bind(subscriber)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn rebuild(pool: &PgPool, attachment: i64, options: &WorkerOptions) {
    sqlx::query("UPDATE attachment_extractions SET extractor_version=$2 WHERE attachment_id=$1")
        .bind(attachment)
        .bind(EXTRACTOR_VERSION - 1)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        run_once(pool, options).await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: attachment,
            pages: 1
        }
    );
}

#[tokio::test]
async fn ocr_failure_retry_and_reindex_never_change_publication_or_generate_mail() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "ocr-regression@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    subscriptions::use_token(&app.state, &app.confirmation_token(email).await, true, now)
        .await
        .unwrap();
    let subscriber: i64 = sqlx::query_scalar("SELECT id FROM subscribers WHERE email=$1")
        .bind(email)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();

    let document = app
        .notice(json!({"title":"Zveřejněný zápis s naskenovanou přílohou"}))
        .await;
    let uploaded = app
        .upload(
            &format!("/api/v1/admin/documents/{document}/attachments"),
            "zapis.pdf",
            include_bytes!("fixtures/search/scan.pdf"),
        )
        .await;
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let attachment = body_json(uploaded).await["id"].as_i64().unwrap();
    assert_eq!(app.publish(document).await.status(), StatusCode::NO_CONTENT);
    let pool = &app.state.pool;
    let before = publication_state(pool, document, subscriber).await;
    assert_eq!(before["source"]["lock_state"], "locked");
    assert_eq!(
        before["source"]["fingerprint"],
        before["source"]["content_sha256"]
    );
    assert!(!before["events"].as_array().unwrap().is_empty());
    assert_eq!(before["announcements"].as_array().unwrap().len(), 1);
    assert_eq!(before["outbox"].as_array().unwrap().len(), 1);
    assert!(before["digest_items"].as_array().unwrap().is_empty());

    let (_failure_dir, failed_options) = extractor(false);
    assert_eq!(
        run_once(pool, &failed_options).await.unwrap(),
        RunOutcome::Failed {
            attachment_id: attachment
        }
    );
    assert_eq!(
        publication_state(pool, document, subscriber).await,
        before,
        "Failed OCR changed publication or mail state"
    );

    sqlx::query("UPDATE attachment_extractions SET next_attempt_at=now()-interval '1 second' WHERE attachment_id=$1")
        .bind(attachment).execute(pool).await.unwrap();
    let (_success_dir, options) = extractor(true);
    assert_eq!(
        run_once(pool, &options).await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: attachment,
            pages: 1
        }
    );
    assert_eq!(
        publication_state(pool, document, subscriber).await,
        before,
        "Successful OCR retry changed publication or mail state"
    );
    rebuild(pool, attachment, &options).await;
    assert_eq!(
        publication_state(pool, document, subscriber).await,
        before,
        "Reindexing changed publication or mail state"
    );

    // Normal publication creates exactly its original digest. Extraction must neither
    // add a second item nor make later preparation perceive a new publication.
    let next_hour = OffsetDateTime::from_unix_timestamp(
        (OffsetDateTime::now_utc().unix_timestamp().div_euclid(3600) + 1) * 3600,
    )
    .unwrap();
    assert!(
        mail::prepare_publications(&app.state, next_hour)
            .await
            .unwrap()
    );
    let prepared = publication_state(pool, document, subscriber).await;
    assert_eq!(prepared["source"], before["source"]);
    assert!(prepared["outbox"].as_array().unwrap().is_empty());
    assert_eq!(prepared["digest_items"].as_array().unwrap().len(), 1);
    assert_eq!(
        prepared["mail_queue"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["purpose"] == "document")
            .count(),
        1
    );

    rebuild(pool, attachment, &options).await;
    assert_eq!(
        publication_state(pool, document, subscriber).await,
        prepared,
        "Reindexing altered the prepared digest"
    );
    assert!(
        !mail::prepare_publications(&app.state, next_hour + Duration::hours(2))
            .await
            .unwrap()
    );
    assert_eq!(
        publication_state(pool, document, subscriber).await,
        prepared,
        "Later mail preparation treated OCR as a publication"
    );
}
