#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};

async fn draft(app: &App) -> i64 {
    let response = app
        .call(
            "POST",
            "/api/v1/admin/documents",
            Some(json!({"title":"Anonymní písemnost","description":"Kontrolní text"})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await["id"].as_i64().unwrap()
}
async fn preview(app: &App, id: i64) -> Value {
    body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/documents/{id}/preview"),
            None,
            true,
        )
        .await,
    )
    .await
}
async fn post(app: &App, id: i64, token: &Value, action: &str) -> axum::response::Response {
    let day = obecni_web::backend::today(OffsetDateTime::now_utc());
    app.call("POST",&format!("/api/v1/admin/documents/{id}/postings"),Some(json!({"token":token,"published_on":day,"withdraw_on":day+Duration::days(20),"after_action":action,"reason":"Testovací schválení"})),true).await
}
async fn file(app: &App, id: i64) -> i64 {
    let r = app
        .upload(
            &format!("/api/v1/admin/documents/{id}/attachments"),
            "test.pdf",
            PDF,
        )
        .await;
    assert_eq!(r.status(), StatusCode::CREATED);
    body_json(r).await["id"].as_i64().unwrap()
}

#[tokio::test]
async fn mandatory_choice_has_no_default_and_preview_is_bound_to_exact_bytes() {
    let app = App::new().await;
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/notices",
            Some(json!({"title":"Bez volby"})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let id = draft(&app).await;
    let stale = preview(&app, id).await;
    file(&app, id).await;
    assert_eq!(
        post(&app, id, &stale["token"], "hide").await.status(),
        StatusCode::CONFLICT
    );
    let fresh = preview(&app, id).await;
    assert_eq!(
        post(&app, id, &fresh["token"], "").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(&app, id, &fresh["token"], "hide").await.status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn catalogue_and_board_share_identity_and_private_archive_files_remain_downloadable_to_admin()
{
    let app = App::new().await;
    let id = draft(&app).await;
    let attachment = file(&app, id).await;
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/publish"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let catalog = body_json(
        app.call("GET", "/api/v1/search?kind=document&state=all", None, false)
            .await,
    )
    .await;
    assert_eq!(catalog["total"], 1);
    let board = body_json(
        app.call(
            "GET",
            "/api/v1/search?kind=notice&state=current",
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(board["total"], 0);
    let p = preview(&app, id).await;
    assert_eq!(
        post(&app, id, &p["token"], "hide").await.status(),
        StatusCode::CREATED
    );
    let search = body_json(
        app.call("GET", "/api/v1/search?q=Anonymni&state=all", None, false)
            .await,
    )
    .await;
    assert_eq!(search["total"], 1);
    assert_eq!(search["items"][0]["id"], id);
    assert_eq!(search["items"][0]["path"], format!("/dokumenty/{id}"));
    for path in [
        format!("/api/v1/admin/documents/{id}"),
        format!("/api/v1/admin/notices/{id}"),
    ] {
        let input = if path.contains("notices") {
            json!({"title":"Změna","retain_attachments":false})
        } else {
            json!({"title":"Změna"})
        };
        assert_eq!(
            app.call("PUT", &path, Some(input), true).await.status(),
            StatusCode::CONFLICT
        );
    }
    assert_eq!(
        app.upload(
            &format!("/api/v1/admin/documents/{id}/attachments"),
            "novy.pdf",
            PDF
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/admin/attachments/{attachment}"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert!(
        sqlx::query("UPDATE documents SET title='Zakázáno' WHERE id=$1")
            .bind(id)
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE attachments SET data='changed'::bytea WHERE id=$1")
            .bind(attachment)
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    let posting: i64 = sqlx::query_scalar("SELECT id FROM document_postings WHERE document_id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/postings/{posting}/withdraw"),
            Some(json!({"reason":"Ukončení testu"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/attachments/{attachment}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/attachments/{attachment}"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    let stored: Vec<u8> = sqlx::query_scalar("SELECT data FROM attachments WHERE id=$1")
        .bind(attachment)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, PDF);
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{id}"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
    let original_hash: String =
        sqlx::query_scalar("SELECT content_sha256 FROM documents WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let correction = app
        .call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/corrections"),
            Some(json!({"kind":"correction","reason":"Oprava textu"})),
            true,
        )
        .await;
    assert_eq!(correction.status(), StatusCode::CREATED);
    let corrected = body_json(correction).await["id"].as_i64().unwrap();
    assert_ne!(id, corrected);
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/documents/{corrected}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let p = preview(&app, id).await;
    assert_eq!(
        post(&app, id, &p["token"], "retain").await.status(),
        StatusCode::CREATED
    );
    let (hash,count):(String,i64)=sqlx::query_as("SELECT content_sha256,(SELECT count(*) FROM document_postings WHERE document_id=$1) FROM documents WHERE id=$1").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(hash, original_hash);
    assert_eq!(count, 2);
}

#[tokio::test]
async fn full_withdrawal_blocks_detail_files_search_alias_and_public_api_immediately() {
    let app = App::new().await;
    let id = draft(&app).await;
    let f = file(&app, id).await;
    let p = preview(&app, id).await;
    assert_eq!(
        post(&app, id, &p["token"], "retain").await.status(),
        StatusCode::CREATED
    );
    sqlx::query(
        "INSERT INTO document_legacy_ids(old_model,old_id,document_id) VALUES('notice',7777,$1)",
    )
    .bind(id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/visibility"),
            Some(json!({"public_access":false,"reason":"Interní citlivý důvod"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    for path in [
        format!("/api/v1/documents/{id}"),
        format!("/api/v1/notices/{id}"),
        format!("/api/v1/attachments/{f}"),
        "/uredni-deska/7777".into(),
    ] {
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    let search = body_json(
        app.call("GET", "/api/v1/search?q=Anonymni&state=all", None, false)
            .await,
    )
    .await;
    assert_eq!(search["total"], 0);
    let detail = obecni_web::catalog::document_from_pool(&app.state.pool, id)
        .await
        .unwrap();
    assert!(detail.is_none());
    assert_eq!(
        app.call("GET", &format!("/api/v1/admin/attachments/{f}"), None, true)
            .await
            .status(),
        StatusCode::OK
    );
    assert!(
        sqlx::query("UPDATE documents SET lock_state='draft' WHERE id=$1")
            .bind(id)
            .execute(&app.state.pool)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn publication_and_concurrent_edit_cannot_publish_unreviewed_content() {
    let app = App::new().await;
    let id = draft(&app).await;
    file(&app, id).await;
    let p = preview(&app, id).await;
    let path = format!("/api/v1/admin/documents/{id}");
    let (published, edited) = tokio::join!(
        post(&app, id, &p["token"], "hide"),
        app.call("PUT", &path, Some(json!({"title":"Souběžná změna"})), true)
    );
    assert!(
        (published.status() == StatusCode::CREATED && edited.status() == StatusCode::CONFLICT)
            || (published.status() == StatusCode::CONFLICT
                && edited.status() == StatusCode::NO_CONTENT)
    );
}

#[tokio::test]
async fn existing_import_is_never_reannounced_and_one_document_uses_one_announcement_identity() {
    let app = App::new().await;
    let id = draft(&app).await;
    sqlx::query(
        "INSERT INTO document_announcements(document_id,origin) VALUES($1,'existing_content')",
    )
    .bind(id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    app.call(
        "POST",
        "/api/v1/subscriptions",
        Some(json!({"email":"reader@example.test"})),
        false,
    )
    .await;
    let token = app.confirmation_token("reader@example.test").await;
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/subscriptions/verify",
            Some(json!({"token":token})),
            false
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_queue")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/publish"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let p = preview(&app, id).await;
    assert_eq!(
        post(&app, id, &p["token"], "hide").await.status(),
        StatusCode::CREATED
    );
    let (mail, outbox): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM mail_queue),(SELECT count(*) FROM publication_outbox)",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(mail, before);
    assert_eq!(outbox, 0);
    let new = draft(&app).await;
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{new}/publish"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let p = preview(&app, new).await;
    assert_eq!(
        post(&app, new, &p["token"], "hide").await.status(),
        StatusCode::CREATED
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM publication_outbox")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn changed_plan_and_removal_respect_reviewed_minimum_with_explicit_incident() {
    let app = App::new().await;
    let id = draft(&app).await;
    let day = obecni_web::backend::today(OffsetDateTime::now_utc());
    let p = preview(&app, id).await;
    let r=app.call("POST",&format!("/api/v1/admin/documents/{id}/postings"),Some(json!({"token":p["token"],"published_on":day,"withdraw_on":day+Duration::days(20),"after_action":"hide","reason":"Schválené pravidlo","review":{"rule":"custom","not_before":day+Duration::days(10),"reviewed":true}})),true).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let posting: i64 = sqlx::query_scalar("SELECT id FROM document_postings WHERE document_id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/postings/{posting}/plan"),
            Some(json!({"withdraw_on":day+Duration::days(5),"reason":"Příliš krátká změna"})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let path = format!("/api/v1/admin/postings/{posting}/withdraw");
    assert_eq!(
        app.call("POST", &path, Some(json!({"reason":"Běžné sejmutí"})), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(json!({"reason":"Výslovně řešený incident","emergency":true})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM document_events WHERE document_id=$1 AND kind='emergency_removal_requested'").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn local_access_block_cancels_future_posting_and_survives_maintenance() {
    let app = App::new().await;
    let id = draft(&app).await;
    let now = OffsetDateTime::now_utc();
    let day = obecni_web::backend::today(now);
    let p = preview(&app, id).await;
    assert_eq!(app.call("POST",&format!("/api/v1/admin/documents/{id}/postings"),Some(json!({"token":p["token"],"published_on":day+Duration::days(1),"withdraw_on":day+Duration::days(20),"after_action":"hide","reason":"Naplánování"})),true).await.status(),StatusCode::CREATED);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/visibility"),
            Some(json!({"public_access":false,"reason":"Místní zákaz obnovení"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    obecni_web::backend::notices::maintenance(&app.state, now + Duration::days(2))
        .await
        .unwrap();
    let (public,blocked,postings):(bool,bool,i64)=sqlx::query_as("SELECT public_access,access_blocked,(SELECT count(*) FROM document_postings p WHERE p.document_id=d.id) FROM documents d WHERE id=$1").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert!(!public);
    assert!(blocked);
    assert_eq!(postings, 0);
}

#[tokio::test]
async fn preview_exposes_missing_file_and_incomplete_document_cannot_be_posted() {
    let app = App::new().await;
    let id = draft(&app).await;
    sqlx::query("INSERT INTO attachments(document_id,name,content_type,size_bytes) VALUES($1,'missing.pdf','application/pdf',42)").bind(id).execute(&app.state.pool).await.unwrap();
    let p = preview(&app, id).await;
    assert_eq!(p["attachments"][0]["available"], false);
    assert_eq!(
        post(&app, id, &p["token"], "hide").await.status(),
        StatusCode::CONFLICT
    );
    let state: String = sqlx::query_scalar("SELECT lock_state FROM documents WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "draft");
}

#[tokio::test]
async fn anonymized_rendition_requires_own_current_preview_and_freezes_without_posting_claim() {
    let app = App::new().await;
    let id = draft(&app).await;
    file(&app, id).await;
    let r = app
        .call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/corrections"),
            Some(json!({"kind":"anonymized","reason":"Samostatné veřejné znění"})),
            true,
        )
        .await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let derived = body_json(r).await["id"].as_i64().unwrap();
    let stale = preview(&app, derived).await;
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/documents/{derived}"),
            Some(json!({"title":"Samostatná anonymizovaná podoba","description":"Veřejný text"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let p = preview(&app, id).await;
    let day = obecni_web::backend::today(OffsetDateTime::now_utc());
    let mut input = json!({"token":p["token"],"published_on":day,"after_action":"anonymized","redacted_document_id":derived,"redacted_token":stale["token"],"reason":"Schválení obou podob"});
    let path = format!("/api/v1/admin/documents/{id}/postings");
    assert_eq!(
        app.call("POST", &path, Some(input.clone()), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    input["redacted_token"] = preview(&app, derived).await["token"].clone();
    assert_eq!(
        app.call("POST", &path, Some(input), true).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/documents/{derived}"),
            Some(json!({"title":"Změna po schválení"})),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_postings WHERE document_id=$1")
            .bind(derived)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{derived}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let posting: i64 = sqlx::query_scalar("SELECT id FROM document_postings WHERE document_id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/postings/{posting}/withdraw"),
            Some(json!({"reason":"Zpřístupnění schváleného znění"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{id}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let detail = body_json(
        app.call("GET", &format!("/api/v1/documents/{derived}"), None, false)
            .await,
    )
    .await;
    assert_eq!(detail["title"], "Samostatná anonymizovaná podoba");
    assert_eq!(detail["postings"].as_array().unwrap().len(), 0);
    let (original_blocked,announcement):(bool,bool)=sqlx::query_as("SELECT access_blocked,EXISTS(SELECT 1 FROM document_announcements WHERE document_id=$2) FROM documents WHERE id=$1").bind(id).bind(derived).fetch_one(&app.state.pool).await.unwrap();
    assert!(original_blocked && announcement);
}
