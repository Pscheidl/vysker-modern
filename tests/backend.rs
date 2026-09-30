#![cfg(feature = "ssr")]
mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::*;
use http_body_util::BodyExt;
use obecni_web::backend::{self, notices, subscriptions};
use serde_json::json;
use time::{
    Duration, OffsetDateTime,
    macros::{date, datetime},
};
use tower::ServiceExt;

#[tokio::test]
async fn admin_requires_session_csrf_and_matching_origin() {
    let app = App::new().await;
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/notices",
            Some(json!({"title":"Neoprávněně"})),
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let no_csrf = Request::builder()
        .method("POST")
        .uri("/api/v1/admin/notices")
        .header("cookie", &app.cookie)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"title":"CSRF"}"#))
        .unwrap();
    assert_eq!(
        app.router.clone().oneshot(no_csrf).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let foreign = Request::builder()
        .method("POST")
        .uri("/api/v1/admin/notices")
        .header("cookie", &app.cookie)
        .header("x-csrf-token", &app.csrf)
        .header("origin", "https://cizi.example")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"title":"CSRF"}"#))
        .unwrap();
    assert_eq!(
        app.router.clone().oneshot(foreign).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let session = app.call("GET", "/api/v1/admin/session", None, true).await;
    assert_eq!(session.headers()["cache-control"], "no-store");
    assert_eq!(body_json(session).await["administrator_id"], app.admin_id);
    assert_eq!(
        app.call("DELETE", "/api/v1/admin/session", None, true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", "/api/v1/admin/session", None, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn default_is_fifteen_days_and_no_file_retention() {
    let app = App::new().await;
    let id = app.notice(json!({})).await;
    let row: (time::Date, time::Date, bool, String) = sqlx::query_as(
        "SELECT published_on,withdraw_on,retain_attachments,status FROM notices WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!((row.1 - row.0).whole_days(), 15);
    assert!(!row.2);
    assert_eq!(row.3, "draft");
    assert_eq!(
        app.call("GET", &format!("/api/v1/notices/{id}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let unlimited = app.notice(json!({"unlimited":true})).await;
    let end: Option<String> = sqlx::query_scalar("SELECT withdraw_on FROM notices WHERE id=$1")
        .bind(unlimited)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(end, None);
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/notices",
            Some(json!({"title":"Chybná lhůta","duration_days":0})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/notices",
            Some(json!({"title":"Chybná lhůta","unlimited":true,"duration_days":15})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn withdrawal_removes_bytes_but_preserves_record_metadata_and_audit() {
    let app = App::new().await;
    let id = app.notice(json!({})).await;
    let uploaded = app
        .upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "vyhlaska.pdf",
            PDF,
        )
        .await;
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let file = body_json(uploaded).await["id"].as_i64().unwrap();
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    let download = app
        .call("GET", &format!("/api/v1/attachments/{file}"), None, false)
        .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["x-content-type-options"], "nosniff");
    assert!(
        download.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(
        &download.into_body().collect().await.unwrap().to_bytes()[..],
        PDF
    );
    assert_eq!(
        app.upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "pozde.pdf",
            PDF
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/notices/{id}/withdraw"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let detail = body_json(
        app.call("GET", &format!("/api/v1/notices/{id}"), None, false)
            .await,
    )
    .await;
    assert_eq!(detail["status"], "archived");
    assert_eq!(detail["attachments"][0]["name"], format!("Příloha #{file}"));
    assert_eq!(detail["attachments"][0]["available"], false);
    let removed: bool = sqlx::query_scalar(
        "SELECT data IS NULL AND removed_at IS NOT NULL FROM attachments WHERE id=$1",
    )
    .bind(file)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(removed);
    assert!(
        sqlx::query("DELETE FROM notices WHERE id=$1")
            .bind(id)
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM attachments WHERE id=$1")
            .bind(file)
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE audit_log SET operation='updated'")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM audit_log")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    let audit = body_json(
        app.call("GET", "/api/v1/admin/audit?limit=100", None, true)
            .await,
    )
    .await;
    assert!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["operation"] == "withdrawn" && v["actor_id"] == app.admin_id)
    );
}

#[tokio::test]
async fn scheduled_publication_and_expiry_are_idempotent_in_prague_time() {
    let app = App::new().await;
    let id = app
        .notice(json!({"published_on":"2026-10-25","duration_days":2,"retain_attachments":true,"review":{"archive_basis":"Zveřejnění veřejných podkladů","archive_until":"2027-01-01"}}))
        .await;
    let file = body_json(
        app.upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "archiv.pdf",
            PDF,
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    notices::publish_at(
        &app.state,
        id,
        app.admin_id,
        datetime!(2026-10-24 12:00 UTC),
    )
    .await
    .unwrap();
    let state: String = sqlx::query_scalar("SELECT status FROM notices WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "scheduled");
    assert_eq!(
        backend::today(datetime!(2026-10-24 22:30 UTC)),
        date!(2026 - 10 - 25)
    );
    notices::maintenance(&app.state, datetime!(2026-10-24 22:30 UTC))
        .await
        .unwrap();
    let state: String = sqlx::query_scalar("SELECT status FROM notices WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "published");
    notices::maintenance(&app.state, datetime!(2026-10-26 23:01 UTC))
        .await
        .unwrap();
    notices::maintenance(&app.state, datetime!(2026-10-26 23:02 UTC))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE operation='withdrawn' AND entity_id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let preserved: bool = sqlx::query_scalar(
        "SELECT data IS NOT NULL AND removed_at IS NULL FROM attachments WHERE id=$1",
    )
    .bind(file)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(preserved);
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn missed_schedule_is_archived_without_announcing_expired_document() {
    let app = App::new().await;
    let id = app
        .notice(json!({"published_on":"2026-10-01","withdraw_on":"2026-10-03"}))
        .await;
    notices::publish_at(
        &app.state,
        id,
        app.admin_id,
        datetime!(2026-09-30 12:00 UTC),
    )
    .await
    .unwrap();
    notices::maintenance(&app.state, datetime!(2026-10-10 12:00 UTC))
        .await
        .unwrap();
    let state: String = sqlx::query_scalar("SELECT status FROM notices WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "archived");
    let sent: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(sent, 0);
}

#[tokio::test]
async fn expired_nonretained_file_is_hidden_even_before_worker_runs() {
    let app = App::new().await;
    let id = app.notice(json!({})).await;
    let file = body_json(
        app.upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "file.pdf",
            PDF,
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    app.publish(id).await;
    sqlx::query(
        "UPDATE notices SET published_on='2020-01-01',withdraw_on='2020-01-16' WHERE id=$1",
    )
    .bind(id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    notices::maintenance(&app.state, OffsetDateTime::now_utc())
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notices WHERE id=$1 AND status='archived'")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn double_opt_in_only_announces_future_documents_and_unsubscribe_cancels_queue() {
    let app = App::new().await;
    let email = "citizen@example.test";
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/subscriptions",
            Some(json!({"email":email})),
            false
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    let token = app.confirmation_token(email).await;
    let draft = app.notice(json!({})).await;
    app.publish(draft).await;
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(queued, 0);
    assert_eq!(
        app.call(
            "GET",
            &format!("/odber/potvrdit?token={token}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    let confirmed: Option<i64> =
        sqlx::query_scalar("SELECT verified_at FROM subscribers WHERE email=$1")
            .bind(email)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(confirmed, None);
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
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/subscriptions/verify",
            Some(json!({"token":token})),
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let published = app.notice(json!({})).await;
    assert_eq!(
        app.publish(published).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.publish(published).await.status(), StatusCode::CONFLICT);
    let bodies: Vec<String> =
        sqlx::query_scalar("SELECT body FROM mail_queue WHERE purpose='document'")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(bodies.len(), 1);
    let unsubscribe = token_from_body(&bodies[0]);
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/subscriptions/verify",
            Some(json!({"token":unsubscribe})),
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/subscriptions/unsubscribe",
            Some(json!({"token":unsubscribe})),
            false
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_queue WHERE cancelled=FALSE AND sent_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
    app.publish(app.notice(json!({})).await).await;
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(queued, 1);
}

#[tokio::test]
async fn verification_tokens_expire_and_resending_revokes_old_link() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "expire@example.test", now)
        .await
        .unwrap();
    let old = app.confirmation_token("expire@example.test").await;
    common::request_subscription(
        &app.state,
        "expire@example.test",
        now + Duration::minutes(1),
    )
    .await
    .unwrap();
    let new = app.confirmation_token("expire@example.test").await;
    assert_ne!(old, new);
    assert!(
        subscriptions::use_token(&app.state, &old, true, now)
            .await
            .is_err()
    );
    assert!(
        subscriptions::use_token(&app.state, &new, true, now + Duration::hours(25))
            .await
            .is_err()
    );
    let stored: String =
        sqlx::query_scalar("SELECT hash FROM subscription_tokens WHERE purpose='verification'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_ne!(stored, new);
}

#[tokio::test]
async fn generic_documents_and_pages_are_private_until_published() {
    let app = App::new().await;
    common::request_subscription(
        &app.state,
        "confirmed@example.test",
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let token = app.confirmation_token("confirmed@example.test").await;
    subscriptions::use_token(&app.state, &token, true, OffsetDateTime::now_utc())
        .await
        .unwrap();
    let response = app
        .call(
            "POST",
            "/api/v1/admin/documents",
            Some(json!({"title":"Formulář","description":"Obecný dokument"})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = body_json(response).await["id"].as_i64().unwrap();
    let uploaded = body_json(
        app.upload(
            &format!("/api/v1/admin/documents/{id}/attachments"),
            "form.pdf",
            PDF,
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{id}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
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
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{id}"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(notifications, 1);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/archive"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/attachments/{uploaded}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let page = json!({"slug":"spolky","title":"Spolky","content":"<script>alert(1)</script>","published":false});
    let response = app
        .call("POST", "/api/v1/admin/pages", Some(page.clone()), true)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = body_json(response).await["id"].as_i64().unwrap();
    assert_eq!(
        app.call("GET", "/api/v1/pages/spolky", None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut page = page;
    page["published"] = json!(true);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/pages/{id}"),
            Some(page),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let response = app.call("GET", "/api/v1/pages/spolky", None, false).await;
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(
        body_json(response).await["content"],
        "<script>alert(1)</script>"
    );
}

#[tokio::test]
async fn invalid_upload_and_failed_mutation_leave_no_partial_records() {
    let app = App::new().await;
    let id = app.notice(json!({})).await;
    let path = format!("/api/v1/admin/notices/{id}/attachments");
    assert_eq!(
        app.upload(&path, "../escape.pdf", PDF).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.upload(&path, "script.svg", b"<svg/>").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.upload(&path, "pretend.pdf", b"<script/>")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(files, 0);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/notices",
            Some(json!({"title":"Chybná kategorie","category_id":9999})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, after);
}

#[tokio::test]
async fn health_and_audit_pagination() {
    let app = App::new().await;
    assert_eq!(
        app.call("GET", "/api/v1/health", None, false)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call("GET", "/api/v1/admin/audit", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call("GET", "/api/v1/admin/audit?limit=10000", None, true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let entries = body_json(
        app.call("GET", "/api/v1/admin/audit?limit=1", None, true)
            .await,
    )
    .await;
    assert_eq!(entries.as_array().unwrap().len(), 1);
}
