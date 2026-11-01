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
        fresh["document"]["current_date"],
        obecni_web::backend::today(OffsetDateTime::now_utc()).to_string()
    );
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
            json!({"title":"Změna","retain_attachments":false,"duration_days":7})
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
async fn duration_computes_dates_and_plan_changes_preserve_actual_history() {
    let app = App::new().await;
    let id = draft(&app).await;
    let day = obecni_web::backend::today(OffsetDateTime::now_utc());
    let p = preview(&app, id).await;
    let r=app.call("POST",&format!("/api/v1/admin/documents/{id}/postings"),Some(json!({"token":p["token"],"published_on":day,"duration_days":7,"after_action":"hide","reason":"Výslovně zadaná doba"})),true).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let (posting, start, planned): (i64, time::Date, time::Date) = sqlx::query_as(
        "SELECT id,started_on,planned_removal_on FROM document_postings WHERE document_id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(start, day);
    assert_eq!(planned, day + Duration::days(8));
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/postings/{posting}/plan"),
            Some(json!({"withdraw_on":null,"reason":"Chybějící datum"})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/postings/{posting}/plan"),
            Some(json!({"withdraw_on":day+Duration::days(5),"reason":"Změna počtu dní"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/postings/{posting}/withdraw"),
            Some(json!({"reason":"Sejmutí obsluhou"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let (start,removed,events):(time::Date,time::Date,i64)=sqlx::query_as("SELECT started_on,removed_on,(SELECT count(*) FROM document_events WHERE document_id=$1 AND kind='plan_changed') FROM document_postings WHERE id=$2").bind(id).bind(posting).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!((start, removed, events), (day, day, 1));
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
    let attachment = p["attachments"][0]["id"].as_i64().unwrap();
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/admin/attachments/{attachment}"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let complete = preview(&app, id).await;
    assert_eq!(complete["attachments"].as_array().unwrap().len(), 0);
    assert_ne!(complete["token"], p["token"]);
    assert_eq!(
        post(&app, id, &complete["token"], "hide").await.status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn automatic_anonymized_release_is_rejected_and_legacy_removal_keeps_copy_private() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let day = obecni_web::backend::today(now);
    for automatic in [false, true] {
        let id = draft(&app).await;
        let original_file = file(&app, id).await;
        let response = app
            .call(
                "POST",
                &format!("/api/v1/admin/documents/{id}/corrections"),
                Some(json!({"kind":"anonymized","reason":"Historická samostatná podoba"})),
                true,
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let derived = body_json(response).await["id"].as_i64().unwrap();
        let p = preview(&app, id).await;
        assert_eq!(
            post(&app, id, &p["token"], "anonymized").await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(app.call("POST", &format!("/api/v1/admin/documents/{id}/postings"), Some(json!({"token":p["token"],"published_on":day,"duration_days":7,"after_action":"hide","redacted_document_id":derived,"redacted_token":"obsolete","reason":"Starý klient"})), true).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let derived_file: i64 =
            sqlx::query_scalar("SELECT id FROM attachments WHERE document_id=$1")
                .bind(derived)
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
        let before: Vec<(i64, Vec<u8>, String)> = sqlx::query_as(
            "SELECT id,data,sha256 FROM attachments WHERE document_id IN ($1,$2) ORDER BY id",
        )
        .bind(id)
        .bind(derived)
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE documents SET lock_state='locked' WHERE id IN ($1,$2)")
            .bind(id)
            .bind(derived)
            .execute(&app.state.pool)
            .await
            .unwrap();
        let removal = if automatic {
            day - Duration::days(1)
        } else {
            day + Duration::days(2)
        };
        sqlx::query("UPDATE documents SET status='published',public_access=TRUE,board_status='published',published_on=$2,withdraw_on=$3,posting_after_action='anonymized',posting_redacted_id=$4 WHERE id=$1").bind(id).bind(day-Duration::days(10)).bind(removal).bind(derived).execute(&app.state.pool).await.unwrap();
        let posting: i64 = sqlx::query_scalar("INSERT INTO document_postings(document_id,origin,certainty,initial_state,started_on,started_at,planned_removal_on,after_action,redacted_document_id,snapshot_sha256) VALUES($1,'local','documented','current',$2,$3,$4,'anonymized',$5,document_fingerprint($1)) RETURNING id").bind(id).bind(day-Duration::days(10)).bind(now-Duration::days(10)).bind(removal).bind(derived).fetch_one(&app.state.pool).await.unwrap();
        assert_eq!(
            app.call(
                "GET",
                &format!("/api/v1/attachments/{original_file}"),
                None,
                false
            )
            .await
            .status(),
            StatusCode::OK
        );
        if automatic {
            obecni_web::backend::notices::maintenance(&app.state, now)
                .await
                .unwrap();
            obecni_web::backend::notices::maintenance(&app.state, now)
                .await
                .unwrap();
        } else {
            assert_eq!(
                app.call(
                    "POST",
                    &format!("/api/v1/admin/postings/{posting}/withdraw"),
                    Some(json!({"reason":"Sejmutí historického nastavení"})),
                    true
                )
                .await
                .status(),
                StatusCode::NO_CONTENT
            );
        }
        for document in [id, derived] {
            assert_eq!(
                app.call("GET", &format!("/api/v1/documents/{document}"), None, false)
                    .await
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        for attachment in [original_file, derived_file] {
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
        }
        let after: Vec<(i64, Vec<u8>, String)> = sqlx::query_as(
            "SELECT id,data,sha256 FROM attachments WHERE document_id IN ($1,$2) ORDER BY id",
        )
        .bind(id)
        .bind(derived)
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(before, after);
        let (blocked,copy_public,copy_announced,copy_postings): (bool,bool,bool,i64) = sqlx::query_as("SELECT access_blocked,(SELECT public_access FROM documents WHERE id=$2),EXISTS(SELECT 1 FROM document_announcements WHERE document_id=$2),(SELECT count(*) FROM document_postings WHERE document_id=$2) FROM documents WHERE id=$1").bind(id).bind(derived).fetch_one(&app.state.pool).await.unwrap();
        assert!(blocked);
        assert!(!copy_public && !copy_announced);
        assert_eq!(copy_postings, 0);
    }
    let queued: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM publication_outbox)+(SELECT count(*) FROM mail_queue)",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(queued, 0);
}

#[tokio::test]
async fn retired_anonymized_schedule_is_cancelled_without_blocking_other_publications() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let day = obecni_web::backend::today(now) + Duration::days(1);
    let id = draft(&app).await;
    let derived = draft(&app).await;
    let ordinary = draft(&app).await;
    for document in [id, ordinary] {
        let p = preview(&app, document).await;
        assert_eq!(app.call("POST", &format!("/api/v1/admin/documents/{document}/postings"), Some(json!({"token":p["token"],"published_on":day,"duration_days":7,"after_action":"hide","reason":"Plánované vyvěšení"})), true).await.status(), StatusCode::CREATED);
    }
    sqlx::query(
        "UPDATE documents SET posting_after_action='anonymized',posting_redacted_id=$2 WHERE id=$1",
    )
    .bind(id)
    .bind(derived)
    .execute(&app.state.pool)
    .await
    .unwrap();
    obecni_web::backend::notices::maintenance(&app.state, now + Duration::days(1))
        .await
        .unwrap();
    let (state,public,locked,announced,events): (String,bool,String,bool,i64) = sqlx::query_as("SELECT board_status,public_access,lock_state,EXISTS(SELECT 1 FROM document_announcements WHERE document_id=$1),(SELECT count(*) FROM notice_events WHERE notice_id=$1 AND kind='schedule_cancelled') FROM documents WHERE id=$1").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(state, "draft");
    assert_eq!(locked, "draft");
    assert!(!public && !announced);
    assert_eq!(events, 1);
    let detail = body_json(
        app.call("GET", &format!("/api/v1/admin/documents/{id}"), None, true)
            .await,
    )
    .await;
    assert!(detail["retain_attachments"].is_null());
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{derived}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{ordinary}"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn unified_editor_requires_explicit_board_choices_and_keeps_one_archive() {
    let app = App::new().await;
    let day = obecni_web::backend::today(OffsetDateTime::now_utc());
    for fields in [
        json!({"title":"Bez doby","board_enabled":true,"published_on":day,"retain_attachments":false}),
        json!({"title":"Bez přístupu","board_enabled":true,"published_on":day,"duration_days":7}),
        json!({"title":"Nula dní","board_enabled":true,"published_on":day,"duration_days":0,"retain_attachments":true}),
    ] {
        assert_eq!(
            app.call("POST", "/api/v1/admin/documents", Some(fields), true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let ordinary = draft(&app).await;
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{ordinary}/publish"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let created=body_json(app.call("POST","/api/v1/admin/documents",Some(json!({"title":"Jednotný dokument","board_enabled":true,"published_on":day,"duration_days":7,"retain_attachments":false})),true).await).await;
    let board = created["id"].as_i64().unwrap();
    let attachment = file(&app, board).await;
    let d = body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/documents/{board}"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(d["board_enabled"], true);
    assert_eq!(d["duration_days"], 7);
    assert_eq!(d["retain_attachments"], false);
    assert!(d["reference_number"].is_null() && d["document_date"].is_null());
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{board}/publish"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let p = preview(&app, board).await;
    assert_eq!(app.call("POST",&format!("/api/v1/admin/documents/{board}/postings"),Some(json!({"token":p["token"],"published_on":day,"duration_days":7,"after_action":"hide","reason":"Potvrzený náhled"})),true).await.status(),StatusCode::CREATED);
    let current = body_json(
        app.call("GET", "/api/v1/admin/documents?state=current", None, true)
            .await,
    )
    .await;
    assert_eq!(current.as_array().unwrap().len(), 1);
    assert_eq!(current[0]["id"], board);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{ordinary}/archive"),
            Some(json!({"reason":"Přesun do společného archivu"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{ordinary}"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
    // The worker is run in the future. Keep this fixture's session alive too.
    sqlx::query("UPDATE sessions SET expires_at=$1 WHERE administrator_id=$2")
        .bind((OffsetDateTime::now_utc() + Duration::days(30)).unix_timestamp())
        .bind(app.admin_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    obecni_web::backend::notices::maintenance(
        &app.state,
        OffsetDateTime::now_utc() + Duration::days(8),
    )
    .await
    .unwrap();
    let archived = body_json(
        app.call("GET", "/api/v1/admin/documents?state=archive", None, true)
            .await,
    )
    .await;
    assert_eq!(
        archived
            .as_array()
            .expect(&format!("archive response: {archived}"))
            .len(),
        2
    );
    assert!(
        archived
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"] == ordinary)
    );
    assert!(
        archived
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"] == board)
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
    let empty = body_json(
        app.call("GET", "/api/v1/admin/documents?state=current", None, true)
            .await,
    )
    .await;
    assert!(empty.as_array().unwrap().is_empty());
    let page = body_json(
        app.call(
            "GET",
            "/api/v1/admin/documents?state=archive&limit=1&offset=1",
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(page.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn old_admin_notice_ids_resolve_namespace_collisions_without_public_leak() {
    let app = App::new().await;
    let ordinary = draft(&app).await;
    let board = app.notice(json!({"title":"Jiný původní model"})).await;
    sqlx::query(
        "INSERT INTO document_legacy_ids(old_model,old_id,document_id) VALUES ('notice',$1,$2)",
    )
    .bind(ordinary)
    .bind(board)
    .execute(&app.state.pool)
    .await
    .unwrap();
    let url = format!("/api/v1/admin/legacy/notices/{ordinary}");
    assert_eq!(
        app.call("GET", &url, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let resolved = body_json(app.call("GET", &url, None, true).await).await;
    assert_eq!(resolved["id"], board);
    assert_ne!(resolved["id"], ordinary);
}

#[test]
fn duration_leaves_every_requested_calendar_day_complete() {
    use obecni_web::backend::registry::posting_end;
    use time::macros::date;
    assert_eq!(
        posting_end(date!(2026 - 11 - 01), Some(15), None).unwrap(),
        date!(2026 - 11 - 17)
    );
    assert_eq!(
        posting_end(date!(2026 - 12 - 31), Some(1), None).unwrap(),
        date!(2027 - 01 - 02)
    );
    // Exact dates remain explicit operational choices, never shifted by migration.
    assert_eq!(
        posting_end(date!(2026 - 11 - 01), None, Some(date!(2026 - 11 - 20))).unwrap(),
        date!(2026 - 11 - 20)
    );
    assert!(posting_end(date!(2026 - 11 - 01), None, None).is_err());
    assert!(posting_end(date!(2026 - 11 - 01), None, Some(date!(2026 - 11 - 02))).is_err());
}

#[tokio::test]
async fn historical_posting_without_removal_date_survives_local_reposting_and_removal() {
    for origin in ["source", "migration"] {
        for automatic in [false, true] {
            let app = App::new().await;
            let now = OffsetDateTime::now_utc();
            let day = obecni_web::backend::today(now);
            let id = draft(&app).await;
            let attachment = file(&app, id).await;
            sqlx::query("UPDATE documents SET public_access=TRUE,status='published',board_status='archived',published_on='2020-01-01',withdraw_on='2020-01-20',source_published_on='2020-01-01',attachments_public=TRUE WHERE id=$1").bind(id).execute(&app.state.pool).await.unwrap();
            sqlx::query("UPDATE documents SET lock_state='locked' WHERE id=$1")
                .bind(id)
                .execute(&app.state.pool)
                .await
                .unwrap();
            let old:i64=sqlx::query_scalar("INSERT INTO document_postings(document_id,origin,certainty,initial_state,started_on,planned_removal_on,after_action,snapshot_sha256,source_url) VALUES($1,$2,'documented','historical','2020-01-01','2020-01-20','retain',document_fingerprint($1),'https://example.test/history') RETURNING id").bind(id).bind(origin).fetch_one(&app.state.pool).await.unwrap();
            sqlx::query("INSERT INTO document_announcements(document_id,origin) VALUES($1,'historical_import')").bind(id).execute(&app.state.pool).await.unwrap();
            let original: Value =
                sqlx::query_scalar("SELECT to_jsonb(p) FROM document_postings p WHERE id=$1")
                    .bind(old)
                    .fetch_one(&app.state.pool)
                    .await
                    .unwrap();
            let archive = body_json(
                app.call(
                    "GET",
                    "/api/v1/search?kind=notice&state=archive",
                    None,
                    false,
                )
                .await,
            )
            .await;
            assert_eq!(archive["total"], 1);
            for (action, input) in [
                (
                    "withdraw",
                    json!({"reason":"Místní úkon nesmí uzavřít převzatou historii"}),
                ),
                (
                    "plan",
                    json!({"withdraw_on":day+Duration::days(30),"reason":"Zdrojový plán nelze přepsat"}),
                ),
            ] {
                assert_eq!(
                    app.call(
                        "POST",
                        &format!("/api/v1/admin/postings/{old}/{action}"),
                        Some(input),
                        true
                    )
                    .await
                    .status(),
                    StatusCode::CONFLICT
                );
            }
            let approved = preview(&app, id).await;
            assert_eq!(
                post(&app, id, &approved["token"], "hide").await.status(),
                StatusCode::CREATED
            );
            let local: i64 = sqlx::query_scalar(
                "SELECT id FROM document_postings WHERE document_id=$1 AND origin='local'",
            )
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
            assert_eq!(
                app.call(
                    "GET",
                    &format!("/api/v1/attachments/{attachment}"),
                    None,
                    false
                )
                .await
                .status(),
                StatusCode::OK
            );
            if automatic {
                // Keep the synthetic administrative session valid across the simulated worker date.
                sqlx::query("UPDATE sessions SET expires_at=$1")
                    .bind((now + Duration::days(30)).unix_timestamp())
                    .execute(&app.state.pool)
                    .await
                    .unwrap();
                obecni_web::backend::notices::maintenance(&app.state, now + Duration::days(21))
                    .await
                    .unwrap();
            } else {
                assert_eq!(
                    app.call(
                        "POST",
                        &format!("/api/v1/admin/postings/{local}/withdraw"),
                        Some(json!({"reason":"Ukončení nového místního vyvěšení"})),
                        true
                    )
                    .await
                    .status(),
                    StatusCode::NO_CONTENT
                );
            }
            let preserved: Value =
                sqlx::query_scalar("SELECT to_jsonb(p) FROM document_postings p WHERE id=$1")
                    .bind(old)
                    .fetch_one(&app.state.pool)
                    .await
                    .unwrap();
            assert_eq!(preserved, original, "{origin}, automatic={automatic}");
            let projection:(String,bool,bool,bool)=sqlx::query_as("SELECT board_status,has_current_posting,has_removed_posting,attachments_public FROM documents WHERE id=$1").bind(id).fetch_one(&app.state.pool).await.unwrap();
            assert_eq!(projection, ("archived".to_string(), false, true, false));
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
            let archive = body_json(
                app.call(
                    "GET",
                    "/api/v1/search?kind=notice&state=archive",
                    None,
                    false,
                )
                .await,
            )
            .await;
            assert_eq!(archive["total"], 1);
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM publication_outbox")
                    .fetch_one(&app.state.pool)
                    .await
                    .unwrap(),
                0
            );
        }
    }
}
