#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::backend::accounts;
use serde_json::json;

const PASSWORD: &str = "Testovaci-dlouhe-heslo-123456!";

#[tokio::test]
async fn accounts_require_authentication_reauthentication_and_protect_last_admin() {
    let app = App::new().await;
    assert_eq!(
        app.call("GET", "/api/v1/admin/accounts", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let path = format!("/api/v1/admin/accounts/{}", app.admin_id);
    assert_eq!(
        app.call(
            "PUT",
            &path,
            Some(json!({"active":false,"current_password":"wrong"})),
            true
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "PUT",
            &path,
            Some(json!({"active":false,"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let result = app.call("POST", "/api/v1/admin/accounts", Some(json!({"email":"Second@vysker.test","password":"Second-long-password-123456!","current_password":PASSWORD})), true).await;
    assert_eq!(result.status(), StatusCode::CREATED);
    let id = body_json(result).await["id"].as_i64().unwrap();
    let list = body_json(app.call("GET", "/api/v1/admin/accounts", None, true).await).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(!list.to_string().contains("password"));
    assert_eq!(
        app.call(
            "PUT",
            &path,
            Some(json!({"active":false,"current_password":PASSWORD})),
            true
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
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"admin@vysker.test","password":PASSWORD})),
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(id > app.admin_id);
    let operations: Vec<String> =
        sqlx::query_scalar("SELECT operation FROM audit_log WHERE entity_type='administrator'")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert!(operations.contains(&"deactivated".into()));
}

#[tokio::test]
async fn password_change_invalidates_every_session_and_operator_recovery_is_audited() {
    let app = App::new().await;
    let new = "Changed-long-password-123456!";
    let response = app
        .call(
            "PUT",
            "/api/v1/admin/password",
            Some(json!({"current_password":PASSWORD,"new_password":new})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.call("GET", "/api/v1/admin/session", None, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"admin@vysker.test","password":PASSWORD})),
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"admin@vysker.test","password":new})),
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    accounts::reset_password(&app.state, "admin@vysker.test", PASSWORD.into())
        .await
        .unwrap();
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    let audit: String =
        sqlx::query_scalar("SELECT operation FROM audit_log ORDER BY id DESC LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(audit, "password_reset_by_operator");
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"admin@vysker.test","password":PASSWORD})),
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn revocation_and_password_validation_do_not_leave_partial_changes() {
    let app = App::new().await;
    assert_eq!(
        app.call(
            "PUT",
            "/api/v1/admin/password",
            Some(json!({"current_password":PASSWORD,"new_password":"short"})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call("GET", "/api/v1/admin/session", None, true)
            .await
            .status(),
        StatusCode::OK
    );
    let path = format!("/api/v1/admin/accounts/{}/sessions", app.admin_id);
    assert_eq!(
        app.call(
            "DELETE",
            &path,
            Some(json!({"current_password":PASSWORD})),
            true
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
}

#[tokio::test]
async fn stored_passwords_use_argon2id_with_independent_random_salts() {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    let app = App::new().await;
    let password = "Testovaci-dlouhe-heslo-123456!";
    let first: String = sqlx::query_scalar("SELECT password_hash FROM administrators WHERE id=$1")
        .bind(app.admin_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    obecni_web::backend::accounts::reset_password(&app.state, "admin@vysker.test", password.into())
        .await
        .unwrap();
    let second: String = sqlx::query_scalar("SELECT password_hash FROM administrators WHERE id=$1")
        .bind(app.admin_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let a = PasswordHash::new(&first).unwrap();
    let b = PasswordHash::new(&second).unwrap();
    assert_eq!(a.algorithm.as_str(), "argon2id");
    assert_eq!(b.algorithm.as_str(), "argon2id");
    assert_eq!(a.version, Some(19));
    assert_ne!(first, second);
    assert_ne!(a.salt.unwrap(), b.salt.unwrap());
    assert!(a.salt.unwrap().as_str().len() >= 22);
    assert!(
        Argon2::default()
            .verify_password(password.as_bytes(), &b)
            .is_ok()
    );
    assert!(
        Argon2::default()
            .verify_password(b"wrong-password", &b)
            .is_err()
    );
}

#[tokio::test]
async fn configurable_minimum_is_enforced_for_account_creation_change_and_recovery() {
    use obecni_web::backend::{self, auth};
    let app = App::new().await;
    let mut config = (*app.state.config).clone();
    config.minimum_password_length = 32;
    let state = backend::Backend::new(app.state.pool.clone(), config);
    let app = App {
        router: backend::router(state.clone()),
        state,
        ..app
    };
    let short = "x".repeat(31);
    assert!(
        auth::create_admin(&app.state, "too-short@vysker.test", short.clone())
            .await
            .is_err()
    );
    assert!(
        accounts::reset_password(&app.state, "admin@vysker.test", short.clone())
            .await
            .is_err()
    );
    let result = app
        .call(
            "PUT",
            "/api/v1/admin/password",
            Some(json!({"current_password":PASSWORD,"new_password":short})),
            true,
        )
        .await;
    assert_eq!(result.status(), StatusCode::BAD_REQUEST);
    let result = app
        .call(
            "POST",
            "/api/v1/admin/accounts",
            Some(json!({"email":"short@vysker.test","current_password":PASSWORD,"password":short})),
            true,
        )
        .await;
    assert_eq!(result.status(), StatusCode::BAD_REQUEST);
    let session = body_json(app.call("GET", "/api/v1/admin/session", None, true).await).await;
    assert_eq!(session["minimum_password_length"], 32);
    // Count characters, not UTF-8 bytes. The upper byte limit remains independent.
    assert!(auth::password_hash("ž".repeat(31), 32).await.is_err());
    assert!(auth::password_hash("ž".repeat(32), 32).await.is_ok());
    assert!(auth::password_hash("x".repeat(1025), 32).await.is_err());
    assert!(
        auth::create_admin(&app.state, "long@vysker.test", "x".repeat(32))
            .await
            .is_ok()
    );
    assert!(
        accounts::reset_password(&app.state, "admin@vysker.test", "x".repeat(32))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn simultaneous_deactivation_keeps_an_active_administrator() {
    let app = App::new().await;
    let other =
        obecni_web::backend::auth::create_admin(&app.state, "second@vysker.test", PASSWORD.into())
            .await
            .unwrap();
    let own_path = format!("/api/v1/admin/accounts/{}", app.admin_id);
    let other_path = format!("/api/v1/admin/accounts/{other}");
    let body = json!({"active":false,"current_password":PASSWORD});
    let (first, second) = tokio::join!(
        app.call("PUT", &own_path, Some(body.clone()), true),
        app.call("PUT", &other_path, Some(body), true)
    );
    assert!(first.status().is_success() || second.status().is_success());
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM administrators WHERE active")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(active, 1);
}
