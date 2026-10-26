#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{App, body_json};
use obecni_web::backend::{Backend, privacy, subscriptions};
use serde_json::json;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

async fn current(app: &App) -> serde_json::Value {
    body_json(app.call("GET", "/api/v1/admin/privacy", None, true).await).await
}

#[tokio::test]
async fn privacy_editor_requires_admin_csrf_and_valid_complete_policy() {
    let app = App::new().await;
    assert_eq!(
        app.call("GET", "/api/v1/admin/privacy", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let initial = current(&app).await;
    let update = json!({"policy":initial["policy"], "fingerprint":initial["fingerprint"]});
    assert_eq!(
        app.call("PUT", "/api/v1/admin/privacy", Some(update.clone()), false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/admin/privacy")
                .header("cookie", &app.cookie)
                .header("content-type", "application/json")
                .body(Body::from(update.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for (field, value) in [
        ("controller_email", json!("invalid")),
        ("processors", json!("")),
        ("consent_text", json!("")),
        ("newsletter_terms", json!("")),
        ("rights_text", json!("x".repeat(10_001))),
        ("approved_on", json!("9999-01-01")),
    ] {
        let mut invalid = update.clone();
        invalid["policy"][field] = value;
        assert_eq!(
            app.call("PUT", "/api/v1/admin/privacy", Some(invalid), true)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{field}"
        );
    }
    let mut invalid = update;
    invalid["policy"]["retention"]["pending_days"] = json!(0);
    assert_eq!(
        app.call("PUT", "/api/v1/admin/privacy", Some(invalid), true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(current(&app).await, initial);
}

#[tokio::test]
async fn saved_privacy_is_live_persistent_and_preserves_original_consent() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "original@example.test", now)
        .await
        .unwrap();
    let original_evidence: (String, String, String) =
        sqlx::query_as("SELECT fingerprint,consent_text,privacy_json FROM consent_notices")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let original = current(&app).await;
    let mut updated = original["policy"].clone();
    updated["version"] = json!("admin-edited-2");
    updated["processors"] = json!("Nový smluvní zpracovatel");
    updated["consent_text"] = json!("Nový text dobrovolného souhlasu s odběrem.");
    updated["newsletter_terms"] = json!("Podmínky odběru\n\n- První podmínka");
    let response = app
        .call(
            "PUT",
            "/api/v1/admin/privacy",
            Some(json!({"policy":updated,"fingerprint":original["fingerprint"]})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let saved = body_json(response).await;
    assert_ne!(saved["fingerprint"], original["fingerprint"]);
    assert_eq!(
        body_json(app.call("GET", "/api/v1/privacy", None, false).await).await,
        saved
    );
    assert_eq!(app.call("POST", "/api/v1/subscriptions", Some(json!({"email":"stale@example.test","consent":{"fingerprint":original["fingerprint"]}})), false).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/subscriptions",
            Some(
                json!({"email":"fresh@example.test","consent":{"fingerprint":saved["fingerprint"]}})
            ),
            false
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    let preserved: (String, String, String) = sqlx::query_as(
        "SELECT fingerprint,consent_text,privacy_json FROM consent_notices WHERE fingerprint=$1",
    )
    .bind(&original_evidence.0)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(preserved, original_evidence);
    let mut config = (*app.state.config).clone();
    config.privacy = None;
    let restarted = Backend::new(app.state.pool.clone(), config);
    assert_eq!(
        privacy::notice(&restarted).await.unwrap().fingerprint,
        saved["fingerprint"].as_str().unwrap()
    );
    let confirmed = app.confirmation_token("original@example.test").await;
    subscriptions::use_token(&restarted, &confirmed, true, now)
        .await
        .unwrap();
    app.publish(app.notice(json!({})).await).await;
    assert!(
        obecni_web::backend::mail::prepare_publications(&restarted, now)
            .await
            .unwrap()
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE operation='updated' AND entity_type='privacy'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn concurrent_edits_conflict_and_saved_retention_is_effective() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "pending@example.test", now - Duration::days(3))
        .await
        .unwrap();
    let original = current(&app).await;
    let mut policy = original["policy"].clone();
    policy["retention"]["pending_days"] = json!(7);
    let input = json!({"policy":policy,"fingerprint":original["fingerprint"]});
    let (first, second) = tokio::join!(
        app.call("PUT", "/api/v1/admin/privacy", Some(input.clone()), true),
        app.call("PUT", "/api/v1/admin/privacy", Some(input), true)
    );
    let statuses = [first.status(), second.status()];
    assert!(statuses.contains(&StatusCode::OK));
    assert!(statuses.contains(&StatusCode::CONFLICT));
    privacy::maintenance(&app.state, now).await.unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscribers WHERE email='pending@example.test'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 1);
    privacy::maintenance(&app.state, now + Duration::days(5))
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscribers WHERE email='pending@example.test'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn editing_terms_never_notifies_existing_subscribers_or_requests_new_consent() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    for email in ["pending-terms@example.test", "confirmed-terms@example.test"] {
        common::request_subscription(&app.state, email, now)
            .await
            .unwrap();
    }
    let token = app.confirmation_token("confirmed-terms@example.test").await;
    subscriptions::use_token(&app.state, &token, true, now)
        .await
        .unwrap();
    let original = current(&app).await;
    assert!(original["policy"]["newsletter_terms"].as_str().unwrap().contains(
        "Vyhrazujeme si právo tyto podmínky změnit. Aktuální znění je vždy dostupné na této stránce."
    ));
    let messages_before: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(m)::text FROM mail_queue m ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    let consent_before: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(c)::text FROM subscription_consents c ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    let evidence_before: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(n)::text FROM consent_notices n ORDER BY fingerprint")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    let mut updated = original["policy"].clone();
    updated["newsletter_terms"] = json!(
        "Upravené podmínky odběru. Vyhrazujeme si právo tyto podmínky změnit. Aktuální znění je vždy dostupné na této stránce."
    );
    updated["version"] = json!("changed-without-email");
    let response = app
        .call(
            "PUT",
            "/api/v1/admin/privacy",
            Some(json!({
                "policy": updated, "fingerprint": original["fingerprint"]
            })),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !obecni_web::backend::mail::prepare_publications(&app.state, now)
            .await
            .unwrap()
    );
    let outbox_count: i64 = sqlx::query_scalar("SELECT count(*) FROM publication_outbox")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(outbox_count, 0);
    let messages_after: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(m)::text FROM mail_queue m ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(messages_after, messages_before);
    let consent_after: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(c)::text FROM subscription_consents c ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(consent_after, consent_before);
    let evidence_after: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(n)::text FROM consent_notices n ORDER BY fingerprint")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(evidence_after, evidence_before);
    let active: bool = sqlx::query_scalar(
        "SELECT verified_at IS NOT NULL AND unsubscribed_at IS NULL FROM subscribers WHERE email='confirmed-terms@example.test'"
    ).fetch_one(&app.state.pool).await.unwrap();
    assert!(active);
    // The new default is only a fallback. Explicitly saved terms are preserved verbatim.
    let saved = current(&app).await;
    assert_eq!(saved["policy"], updated);
}
