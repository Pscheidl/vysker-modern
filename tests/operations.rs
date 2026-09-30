#![cfg(feature = "ssr")]
mod common;
use axum::http::{HeaderMap, StatusCode};
use common::*;
use obecni_web::backend::{self, auth, server};
use serde_json::json;
use time::OffsetDateTime;

#[test]
fn proxy_headers_are_accepted_only_from_the_configured_peer() {
    let mut headers = HeaderMap::new();
    headers.insert("x-real-ip", "198.51.100.23".parse().unwrap());
    let proxy = "172.30.64.2".parse().unwrap();
    assert_eq!(
        auth::client_ip(Some(proxy), &headers, Some(proxy)),
        "198.51.100.23"
    );
    assert_eq!(
        auth::client_ip(Some("203.0.113.10".parse().unwrap()), &headers, Some(proxy)),
        "203.0.113.10"
    );
    assert_eq!(auth::client_ip(Some(proxy), &headers, None), "172.30.64.2");
    assert_eq!(auth::client_ip(None, &headers, None), "local");
    headers.insert("x-real-ip", "198.51.100.23, 127.0.0.1".parse().unwrap());
    assert_eq!(
        auth::client_ip(Some(proxy), &headers, Some(proxy)),
        "172.30.64.2"
    );
}
#[tokio::test]
async fn mail_history_is_private_and_contains_no_tokens_or_bodies() {
    let app = App::new().await;
    common::request_subscription(
        &app.state,
        "read-only@vysker.test",
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(
        app.call("GET", "/api/v1/admin/mail", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = app.call("GET", "/api/v1/admin/mail", None, true).await;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let records = body_json(response).await;
    assert_eq!(records[0]["email"], "read-only@vysker.test");
    assert_eq!(records[0]["status"], "pending");
    assert!(records[0].get("body").is_none());
    assert!(!records.to_string().contains("token="));
}
#[tokio::test]
async fn readiness_detects_a_stalled_maintenance_worker() {
    let app = App::new().await;
    assert_eq!(
        app.call("GET", "/api/v1/ready", None, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.state.last_maintenance.store(
        OffsetDateTime::now_utc().unix_timestamp(),
        std::sync::atomic::Ordering::Relaxed,
    );
    assert_eq!(
        app.call("GET", "/api/v1/ready", None, false).await.status(),
        StatusCode::OK
    );
    app.state.last_maintenance.store(
        OffsetDateTime::now_utc().unix_timestamp() - 121,
        std::sync::atomic::Ordering::Relaxed,
    );
    assert_eq!(
        app.call("GET", "/api/v1/ready", None, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
#[tokio::test]
async fn production_requires_real_information_pages_and_preserves_their_routes() {
    let app = App::new().await;
    let mut config = (*app.state.config).clone();
    config.production = true;
    let prod = backend::Backend::new(app.state.pool.clone(), config);
    assert!(server::check_launch_content(&prod).await.is_err());
    let mut first = 0;
    for slug in server::REQUIRED_PAGES {
        let response=app.call("POST","/api/v1/admin/pages",Some(json!({"slug":slug,"title":slug,"content":"Schválený obsah v testovací databázi","published":true})),true).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        if first == 0 {
            first = body_json(response).await["id"].as_i64().unwrap();
        }
    }
    server::check_launch_content(&prod).await.unwrap();
    let prod_app = App {
        router: backend::router(prod),
        ..app
    };
    let response=prod_app.call("PUT",&format!("/api/v1/admin/pages/{first}"),Some(json!({"slug":"kontakt","title":"Kontakt","content":"Aktualizace","published":false,"expected_version":1})),true).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}
