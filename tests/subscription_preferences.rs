#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, body_json};
use http_body_util::BodyExt;
use obecni_web::backend::{auth, subscriptions};
use time::OffsetDateTime;
use tower::ServiceExt;

async fn active_subscription(app: &App) -> (i64, i64, String) {
    subscription(app, "private-settings@example.test", true).await
}

async fn subscription(app: &App, email: &str, confirmed: bool) -> (i64, i64, String) {
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    if confirmed {
        let confirmation = app.confirmation_token(email).await;
        subscriptions::use_token(&app.state, &confirmation, true, now)
            .await
            .unwrap();
    }
    let (subscriber, consent): (i64, i64) = sqlx::query_as(
        "SELECT s.id,c.id FROM subscribers s JOIN subscription_consents c ON c.subscriber_id=s.id WHERE s.email=$1",
    )
    .bind(email)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    let token = auth::token();
    sqlx::query(
        "INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id)
         VALUES ($1,$2,'unsubscribe',$3,$4)",
    )
    .bind(auth::hash(&token))
    .bind(subscriber)
    .bind(now.unix_timestamp() + 86400)
    .bind(consent)
    .execute(&app.state.pool)
    .await
    .unwrap();
    (subscriber, consent, token)
}

async fn category(app: &App, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO categories(name,sort_order) VALUES ($1,0) RETURNING id")
        .bind(name)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn form(app: &App, body: String, origin: Option<&str>) -> Response {
    form_with_fetch_metadata(app, body, origin, None, None).await
}

async fn form_with_fetch_metadata(
    app: &App,
    body: String,
    origin: Option<&str>,
    fetch_site: Option<&str>,
    fetch_mode: Option<&str>,
) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri("/odber/nastaveni")
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    if let Some(fetch_site) = fetch_site {
        request = request.header("sec-fetch-site", fetch_site);
    }
    if let Some(fetch_mode) = fetch_mode {
        request = request.header("sec-fetch-mode", fetch_mode);
    }
    app.router
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn preferences(app: &App, id: i64) -> (bool, Vec<i64>, bool, bool) {
    sqlx::query_as(
        "SELECT all_notice_categories,notice_category_ids,uncategorized_notices,documents FROM subscribers WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn settings_get_is_private_read_only_and_shows_default_all_topics() {
    let app = App::new().await;
    let (id, _, token) = active_subscription(&app).await;
    let category_id = category(&app, "Záměry <obce> & pozemky").await;
    let before = preferences(&app, id).await;
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let response = app
        .call(
            "GET",
            &format!("/odber/nastaveni?token={token}"),
            None,
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Záměry &lt;obce&gt; &amp; pozemky"));
    assert!(html.contains(&format!("name=\"category_{category_id}\"")));
    for field in [
        "all_notice_categories",
        "uncategorized_notices",
        "documents",
    ] {
        assert!(html.contains(&format!("name=\"{field}\" value=\"on\" checked")));
    }
    assert!(!html.contains("private-settings@example.test"));
    assert_eq!(preferences(&app, id).await, before);
    assert_eq!(before, (true, vec![], true, true));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        audits
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM subscription_tokens WHERE hash=$1")
            .bind(auth::hash(&token))
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn saved_selection_reuses_link_and_export_preserves_original_consent() {
    let app = App::new().await;
    let (id, _, token) = active_subscription(&app).await;
    let first = category(&app, "Veřejné vyhlášky").await;
    let second = category(&app, "Rozpočty").await;
    let response = form(
        &app,
        format!("token={token}&category_{second}=on&category_{first}=on&documents=on"),
        Some("http://localhost:3000"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&html).contains("Výběr je uložený."));
    assert_eq!(
        preferences(&app, id).await,
        (false, vec![first, second], false, true)
    );
    let evidence = body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/subscribers/{id}/export"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(evidence["subscriber"]["all_notice_categories"], false);
    assert_eq!(
        evidence["subscriber"]["notice_category_ids"],
        serde_json::json!([first, second])
    );
    assert_eq!(evidence["consents"][0]["all_notice_categories"], true);
    assert_eq!(
        evidence["consents"][0]["notice_category_ids"],
        serde_json::json!([])
    );
    assert_eq!(evidence["consents"][0]["documents"], true);
    assert!(
        evidence["audit_history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["operation"] == "subscription_preferences_updated")
    );
    // The same link still works for settings and complete withdrawal.
    assert_eq!(
        form(
            &app,
            format!("token={token}&uncategorized_notices=on"),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(preferences(&app, id).await, (false, vec![], true, false));
    subscriptions::use_token(&app.state, &token, false, OffsetDateTime::now_utc())
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "GET",
            &format!("/odber/nastaveni?token={token}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn empty_unknown_and_foreign_origin_updates_leave_preferences_unchanged() {
    let app = App::new().await;
    let (id, _, token) = active_subscription(&app).await;
    let before = preferences(&app, id).await;
    for input in [
        String::new(),
        "&category_999999999=on".into(),
        "&category_999999999=on&documents=on".into(),
        "&category_-1=on".into(),
        "&category_invalid=on".into(),
        "&unexpected=on".into(),
        "&documents=false".into(),
    ] {
        let response = form(&app, format!("token={token}{input}"), None).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{input}");
        if input.is_empty() || input.contains("category_999999999") {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let html = String::from_utf8_lossy(&body);
            assert!(html.contains("role=\"alert\""));
            assert!(html.contains("Uložit výběr"));
            assert!(!html.contains("name=\"all_notice_categories\" value=\"on\" checked"));
            assert_eq!(
                html.contains("name=\"documents\" value=\"on\" checked"),
                input.contains("documents=on")
            );
        }
        assert_eq!(preferences(&app, id).await, before);
    }
    assert_eq!(
        form(
            &app,
            format!("token={token}&documents=on"),
            Some("https://other.example")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(preferences(&app, id).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_log WHERE operation='subscription_preferences_updated'"
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn null_origin_navigation_requires_same_origin_fetch_metadata_and_valid_token() {
    let app = App::new().await;
    let (id, _, token) = active_subscription(&app).await;
    let before = preferences(&app, id).await;
    for (site, mode) in [
        (None, Some("navigate")),
        (Some("cross-site"), Some("navigate")),
        (Some("same-site"), Some("navigate")),
        (Some("same-origin"), None),
        (Some("same-origin"), Some("cors")),
    ] {
        let response = form_with_fetch_metadata(
            &app,
            format!("token={token}&documents=on"),
            Some("null"),
            site,
            mode,
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{site:?} {mode:?}"
        );
        assert_eq!(preferences(&app, id).await, before);
    }
    let response = form_with_fetch_metadata(
        &app,
        format!("token={}&documents=on", auth::token()),
        Some("null"),
        Some("same-origin"),
        Some("navigate"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(preferences(&app, id).await, before);
    let response = form_with_fetch_metadata(
        &app,
        format!("token={token}&documents=on"),
        Some("null"),
        Some("same-origin"),
        Some("navigate"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(preferences(&app, id).await, (false, vec![], false, true));
}

#[tokio::test]
async fn settings_require_live_unsubscribe_token_for_current_confirmed_consent() {
    let app = App::new().await;
    let (id, consent, token) = active_subscription(&app).await;
    let before = preferences(&app, id).await;
    for invalid in ["invalid".to_owned(), auth::token()] {
        assert_eq!(
            app.call(
                "GET",
                &format!("/odber/nastaveni?token={invalid}"),
                None,
                false
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            form(&app, format!("token={invalid}&documents=on"), None)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for invalid_state in [
        "UPDATE subscription_tokens SET expires_at=0",
        "UPDATE subscription_tokens SET purpose='verification'",
        "UPDATE subscription_tokens SET consent_id=NULL",
        "UPDATE subscribers SET verified_at=NULL",
        "UPDATE subscribers SET unsubscribed_at=1",
    ] {
        sqlx::query(invalid_state)
            .execute(&app.state.pool)
            .await
            .unwrap();
        for response in [
            app.call(
                "GET",
                &format!("/odber/nastaveni?token={token}"),
                None,
                false,
            )
            .await,
            form(&app, format!("token={token}&documents=on"), None).await,
        ] {
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{invalid_state}"
            );
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
        assert_eq!(preferences(&app, id).await, before);
        sqlx::query(
            "UPDATE subscription_tokens SET expires_at=$1,purpose='unsubscribe',consent_id=$2",
        )
        .bind(OffsetDateTime::now_utc().unix_timestamp() + 86400)
        .bind(consent)
        .execute(&app.state.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE subscribers SET verified_at=1,unsubscribed_at=NULL")
            .execute(&app.state.pool)
            .await
            .unwrap();
    }
    for state in ["pending", "withdrawn", "superseded"] {
        let (subscriber, consent, token) =
            subscription(&app, &format!("{state}@example.test"), state != "pending").await;
        let query = match state {
            "withdrawn" => Some("UPDATE subscription_consents SET withdrawn_at=$1 WHERE id=$2"),
            "superseded" => Some("UPDATE subscription_consents SET superseded_at=$1 WHERE id=$2"),
            _ => None,
        };
        if let Some(query) = query {
            sqlx::query(query)
                .bind(OffsetDateTime::now_utc().unix_timestamp())
                .bind(consent)
                .execute(&app.state.pool)
                .await
                .unwrap();
        } else {
            // Even a legacy verified flag cannot authorize an unconfirmed consent.
            sqlx::query("UPDATE subscribers SET verified_at=1 WHERE id=$1")
                .bind(subscriber)
                .execute(&app.state.pool)
                .await
                .unwrap();
        }
        assert_eq!(
            app.call(
                "GET",
                &format!("/odber/nastaveni?token={token}"),
                None,
                false
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "{state}"
        );
        assert_eq!(
            form(&app, format!("token={token}&documents=on"), None)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{state}"
        );
        assert_eq!(preferences(&app, subscriber).await, before);
    }
}

#[tokio::test]
async fn a_new_consent_does_not_reauthorize_an_old_management_link() {
    let app = App::new().await;
    let (id, consent, token) = active_subscription(&app).await;
    sqlx::query("UPDATE subscription_consents SET withdrawn_at=$2 WHERE id=$1")
        .bind(consent)
        .bind(OffsetDateTime::now_utc().unix_timestamp())
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO subscription_consents(subscriber_id,notice_fingerprint,requested_at,confirmed_at)
         SELECT subscriber_id,notice_fingerprint,requested_at,confirmed_at FROM subscription_consents WHERE id=$1",
    )
    .bind(consent)
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        form(&app, format!("token={token}&documents=on"), None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(preferences(&app, id).await, (true, vec![], true, true));
}
