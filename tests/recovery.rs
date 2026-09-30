#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json, token_from_body};
use obecni_web::backend::{accounts, mail, recovery};
use serde_json::json;
use time::{Duration, OffsetDateTime};

async fn request(app: &App) -> String {
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/password-recovery",
            Some(json!({"email":"admin@vysker.test"})),
            false
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    let body: String =
        sqlx::query_scalar("SELECT body FROM recovery_mail ORDER BY id DESC LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    token_from_body(&body)
}
#[tokio::test]
async fn reset_is_single_use_and_revokes_sessions_and_all_tokens() {
    let app = App::new().await;
    let unknown = app
        .call(
            "POST",
            "/api/v1/admin/password-recovery",
            Some(json!({"email":"missing@vysker.test"})),
            false,
        )
        .await;
    assert_eq!(unknown.status(), StatusCode::ACCEPTED);
    assert_eq!(body_json(unknown).await, json!({"accepted":true}));
    let token = request(&app).await;
    let other = request(&app).await;
    let hashes: Vec<String> = sqlx::query_scalar("SELECT hash FROM password_resets")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert!(!hashes.contains(&token));
    let path = "/api/v1/admin/password-reset";
    assert_eq!(
        app.call("GET", path, None, false).await.status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        app.call(
            "POST",
            path,
            Some(json!({"token":token,"password":"short"})),
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "POST",
            path,
            Some(json!({"token":token,"password":"A-new-long-recovery-password-123"})),
            false
        )
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
    for token in [token, other] {
        assert_eq!(
            app.call(
                "POST",
                path,
                Some(json!({"token":token,"password":"A-new-long-recovery-password-123"})),
                false
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/login",
            Some(
                json!({"email":"admin@vysker.test","password":"A-new-long-recovery-password-123"})
            ),
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE operation='password_recovered'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM recovery_mail WHERE body IS NOT NULL")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}
#[tokio::test]
async fn expiry_operator_reset_and_deactivation_invalidate_recovery() {
    let app = App::new().await;
    let token = request(&app).await;
    sqlx::query("UPDATE password_resets SET expires_at=0")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/password-reset",
            Some(json!({"token":token,"password":"A-new-long-recovery-password-123"})),
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    recovery::maintenance(&app.state, OffsetDateTime::now_utc())
        .await
        .unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT delivery_expired_at IS NOT NULL FROM recovery_mail ORDER BY id LIMIT 1"
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
    );
    let _ = request(&app).await;
    accounts::reset_password(
        &app.state,
        "admin@vysker.test",
        "Operator-recovery-password-123".into(),
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM password_resets")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE administrators SET active=FALSE")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/password-recovery",
            Some(json!({"email":"admin@vysker.test"})),
            false
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM password_resets")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
    recovery::maintenance(&app.state, OffsetDateTime::now_utc() + Duration::days(31))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recovery_mail")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn smtp_failures_retry_without_losing_security_mail_and_expired_mail_is_cancelled() {
    let mut app = App::new().await;
    let mut cfg = (*app.state.config).clone();
    cfg.smtp_port = 9;
    cfg.privacy = None;
    app.state = obecni_web::backend::Backend::new(app.state.pool.clone(), cfg);
    let _ = request(&app).await;
    let now = OffsetDateTime::now_utc();
    let smtp = mail::transport(&app.state).unwrap();
    assert!(mail::deliver_one(&app.state, &smtp, now).await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT attempts FROM recovery_mail")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        1
    );
    assert!(!mail::deliver_one(&app.state, &smtp, now).await.unwrap());
    assert!(
        mail::deliver_one(&app.state, &smtp, now + Duration::hours(1))
            .await
            .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT cancelled AND body IS NULL FROM recovery_mail")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "requires local Mailpit, run scripts/test.sh --smtp"]
async fn mailpit_accepts_recovery_and_history_excludes_the_link() {
    let mut app = App::new().await;
    let mut cfg = (*app.state.config).clone();
    cfg.smtp_port = std::env::var("TEST_SMTP_PORT")
        .unwrap_or_else(|_| "1025".into())
        .parse()
        .unwrap();
    cfg.privacy = None;
    app.state = obecni_web::backend::Backend::new(app.state.pool.clone(), cfg);
    let token = request(&app).await;
    let transport = mail::transport(&app.state).unwrap();
    assert!(
        mail::deliver_one(&app.state, &transport, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT sent_at IS NOT NULL AND body IS NULL FROM recovery_mail"
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
    );
    let history = body_json(app.call("GET", "/api/v1/admin/mail", None, true).await).await;
    assert_eq!(history[0]["purpose"], "recovery");
    assert_eq!(history[0]["status"], "sent");
    assert!(!history.to_string().contains(&token));
}
