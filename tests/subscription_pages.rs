#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::App;
use http_body_util::BodyExt;
use obecni_web::backend::{self, Backend, auth, mail};
use serde_json::json;
use time::OffsetDateTime;
use tower::ServiceExt;

async fn html(response: Response, status: StatusCode) -> String {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("class=\"site-header\""));
    assert!(html.contains("class=\"site-footer page-width\""));
    assert!(html.contains("class=\"subscription-card\""));
    assert!(html.contains("rel=\"stylesheet\""));
    assert!(html.contains("/pkg/obecni-web"));
    assert!(html.contains("<script src=\"/theme.js\"></script>"));
    assert!(!html.contains("Přihlášení k účtu není potřeba."));
    html
}

async fn submit(app: &App, path: &str, token: &str) -> Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("token={token}")))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn native_subscription_pages_keep_scanners_read_only_and_complete_without_javascript() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "native-pages@example.test", now)
        .await
        .unwrap();
    let verification: String =
        sqlx::query_scalar("SELECT body FROM mail_queue WHERE purpose='verification'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(verification.ends_with("http://localhost:3000/ochrana-udaju"));
    let token = app.confirmation_token("native-pages@example.test").await;
    let confirmation = html(
        app.call(
            "GET",
            &format!("/odber/potvrdit?token={token}"),
            None,
            false,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert!(confirmation.contains("<h1>Potvrdit odběr novinek</h1>"));
    let confirmed: Option<i64> =
        sqlx::query_scalar("SELECT confirmed_at FROM subscription_consents")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(confirmed.is_none());
    let complete = html(
        submit(&app, "/odber/potvrdit", &token).await,
        StatusCode::OK,
    )
    .await;
    assert!(complete.contains("<h1>Odběr je potvrzený.</h1>"));
    app.publish(app.notice(json!({})).await).await;
    assert!(
        mail::prepare_publications(&app.state, now + time::Duration::hours(1))
            .await
            .unwrap()
    );
    let notification: String =
        sqlx::query_scalar("SELECT body FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let token = common::token_from_body(&notification);
    assert!(notification.ends_with(&format!(
        "http://localhost:3000/odber/nastaveni?token={token}"
    )));
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    for route in ["odhlasit", "nastaveni"] {
        let page = html(
            app.call("GET", &format!("/odber/{route}?token={token}"), None, false)
                .await,
            StatusCode::OK,
        )
        .await;
        assert!(!page.contains("native-pages@example.test"));
        assert!(page.contains(&format!("name=\"token\" value=\"{token}\"")));
    }
    assert_eq!(
        before,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
    );
    let unsubscribed: Option<i64> = sqlx::query_scalar("SELECT unsubscribed_at FROM subscribers")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(unsubscribed.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM subscription_tokens WHERE hash=$1")
            .bind(auth::hash(&token))
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        1
    );
    let complete = html(
        submit(&app, "/odber/odhlasit", &token).await,
        StatusCode::OK,
    )
    .await;
    assert!(complete.contains("<h1>Odběr je odhlášený.</h1>"));
    let unsubscribed: Option<i64> = sqlx::query_scalar("SELECT unsubscribed_at FROM subscribers")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(unsubscribed.is_some());
    let used = html(
        app.call(
            "GET",
            &format!("/odber/odhlasit?token={token}"),
            None,
            false,
        )
        .await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(used.contains("<h1>Odkaz není dostupný</h1>"));
}

#[tokio::test]
async fn invalid_bearer_links_keep_the_site_design_and_never_offer_a_working_form() {
    let app = App::new().await;
    for route in ["potvrdit", "odhlasit", "nastaveni"] {
        for token in ["invalid".to_owned(), "a".repeat(64)] {
            let page = html(
                app.call("GET", &format!("/odber/{route}?token={token}"), None, false)
                    .await,
                StatusCode::BAD_REQUEST,
            )
            .await;
            assert!(page.contains("<h1>Odkaz není dostupný</h1>"));
            assert!(!page.contains("<form"));
        }
    }
}

#[tokio::test]
async fn confirmation_renders_the_saved_terms_and_rights_without_active_html() {
    let mut app = App::new().await;
    let mut config = (*app.state.config).clone();
    let policy = config.privacy.as_mut().unwrap();
    policy.newsletter_terms =
        "**Podmínky při přihlášení**\n\n<script>alert('terms')</script>".into();
    policy.rights_text = "Práva platná při přihlášení.".into();
    app.state = Backend::new(app.state.pool.clone(), config.clone());
    common::request_subscription(
        &app.state,
        "historical-terms@example.test",
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let token = app
        .confirmation_token("historical-terms@example.test")
        .await;
    let policy = config.privacy.as_mut().unwrap();
    policy.newsletter_terms = "Nové podmínky.".into();
    policy.rights_text = "Nový popis práv.".into();
    app.state = Backend::new(app.state.pool.clone(), config);
    app.router = backend::router(app.state.clone());
    let page = html(
        app.call(
            "GET",
            &format!("/odber/potvrdit?token={token}"),
            None,
            false,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert!(page.contains("<strong>Podmínky při přihlášení</strong>"));
    assert!(page.contains("Práva platná při přihlášení."));
    assert!(!page.contains("<script>alert"));
    assert!(!page.contains("Nové podmínky."));
    assert!(!page.contains("Nový popis práv."));
}
