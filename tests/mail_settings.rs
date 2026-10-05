#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{App, body_json};
use obecni_web::{
    backend::{self, Backend, mail, mail_settings},
    config::Config,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;

const PATH: &str = "/api/v1/admin/mail-settings";
const TEST_PATH: &str = "/api/v1/admin/mail-settings/test";
const ADMIN_PASSWORD: &str = "Testovaci-dlouhe-heslo-123456!";
const APP_PASSWORD: &str = "abcdefghijklmnop";

async fn app() -> (App, TempDir) {
    let app = App::new().await;
    let directory = tempfile::tempdir().unwrap();
    let mut config = (*app.state.config).clone();
    config.mail_settings_key_file = directory.path().join("mail-settings.key");
    (reconfigure(app, config), directory)
}

fn reconfigure(app: App, config: Config) -> App {
    let state = Backend::new(app.state.pool.clone(), config);
    App {
        router: backend::router(state.clone()),
        state,
        ..app
    }
}

fn google() -> Value {
    json!({
        "provider": "google",
        "username": "sender@gmail.com",
        "sender_name": "Obec Vyskeř",
        "password": APP_PASSWORD,
        "current_password": ADMIN_PASSWORD,
    })
}

async fn save(app: &App, input: Value) -> Value {
    let response = app.call("PUT", PATH, Some(input), true).await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

fn assert_no_secrets(value: &Value) {
    let serialized = value.to_string();
    assert!(!serialized.contains(APP_PASSWORD));
    assert!(!serialized.contains(ADMIN_PASSWORD));
    assert!(value.get("password").is_none());
    assert!(value.get("smtp_password").is_none());
    assert!(value.get("password_encrypted").is_none());
}

#[tokio::test]
async fn settings_and_test_require_authentication_csrf_and_reauthentication() {
    let (app, _directory) = app().await;
    for (method, path, body) in [
        ("GET", PATH, None),
        ("PUT", PATH, Some(google())),
        (
            "POST",
            TEST_PATH,
            Some(json!({"current_password": ADMIN_PASSWORD})),
        ),
    ] {
        assert_eq!(
            app.call(method, path, body, false).await.status(),
            StatusCode::UNAUTHORIZED,
        );
    }
    for (method, path, body) in [
        ("PUT", PATH, google()),
        (
            "POST",
            TEST_PATH,
            json!({"current_password": ADMIN_PASSWORD}),
        ),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("cookie", &app.cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let mut wrong_password = body;
        wrong_password["current_password"] = json!("wrong-password");
        assert_eq!(
            app.call(method, path, Some(wrong_password), true)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_settings")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0,
    );
    assert!(!app.state.config.mail_settings_key_file.exists());
}

#[tokio::test]
async fn google_credentials_are_normalized_encrypted_and_survive_new_backend() {
    let (app, _directory) = app().await;
    let mut input = google();
    input["username"] = json!(" Sender@Gmail.Com ");
    input["password"] = json!("abcd efgh ijkl mnop");
    let saved = save(&app, input).await;
    assert_eq!(saved["provider"], "google");
    assert_eq!(saved["username"], "sender@gmail.com");
    assert_eq!(saved["sender_name"], "Obec Vyskeř");
    assert_eq!(saved["password_configured"], true);
    assert_eq!(saved["smtp_host"], "smtp.gmail.com");
    assert_eq!(saved["smtp_port"], 587);
    assert_eq!(saved["smtp_tls"], "starttls");
    assert_no_secrets(&saved);

    let config = mail_settings::effective_config(&app.state).await.unwrap();
    assert_eq!(config.smtp_username.as_deref(), Some("sender@gmail.com"));
    assert_eq!(config.smtp_password.as_deref(), Some(APP_PASSWORD));
    assert!(config.email_from.contains("sender@gmail.com"));
    let encrypted: Vec<u8> =
        sqlx::query_scalar("SELECT password_encrypted FROM mail_settings WHERE id=1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(encrypted.len() > APP_PASSWORD.len());
    assert!(
        !encrypted
            .windows(APP_PASSWORD.len())
            .any(|part| part == APP_PASSWORD.as_bytes())
    );
    let audit: String =
        sqlx::query_scalar("SELECT coalesce(json_agg(audit_log)::text, '') FROM audit_log")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(!audit.contains(APP_PASSWORD));
    assert!(!audit.contains(ADMIN_PASSWORD));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&app.state.config.mail_settings_key_file)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let original_config = (*app.state.config).clone();
    let app = reconfigure(app, original_config);
    let fetched = body_json(app.call("GET", PATH, None, true).await).await;
    assert_eq!(fetched, saved);
    assert_no_secrets(&fetched);
    let restored = mail_settings::effective_config(&app.state).await.unwrap();
    assert_eq!(restored.smtp_password.as_deref(), Some(APP_PASSWORD));
}

#[tokio::test]
async fn blank_password_preserves_only_the_same_saved_google_account() {
    let (app, _directory) = app().await;
    save(&app, google()).await;
    for password in [Some(""), None] {
        let mut input = google();
        input["username"] = json!(" Sender@Gmail.Com ");
        input["sender_name"] = json!("Nový název obce");
        if let Some(password) = password {
            input["password"] = json!(password);
        } else {
            input.as_object_mut().unwrap().remove("password");
        }
        save(&app, input.clone()).await;
        let config = mail_settings::effective_config(&app.state).await.unwrap();
        assert_eq!(config.smtp_password.as_deref(), Some(APP_PASSWORD));
        input["username"] = json!("another@gmail.com");
        assert_eq!(
            app.call("PUT", PATH, Some(input), true).await.status(),
            StatusCode::BAD_REQUEST,
        );
        let unchanged = mail_settings::effective_config(&app.state).await.unwrap();
        assert_eq!(unchanged.smtp_username.as_deref(), Some("sender@gmail.com"));
    }
    let mut replacement = google();
    replacement["password"] = json!("qrst uvwx yzab cdef");
    save(&app, replacement).await;
    assert_eq!(
        mail_settings::effective_config(&app.state)
            .await
            .unwrap()
            .smtp_password
            .as_deref(),
        Some("qrstuvwxyzabcdef"),
    );
}

#[tokio::test]
async fn malformed_google_settings_never_replace_valid_credentials() {
    let (app, _directory) = app().await;
    save(&app, google()).await;
    for (field, value) in [
        ("provider", "unsupported"),
        ("username", "not-an-email"),
        ("password", "short"),
        ("password", "1234567890123456"),
        ("password", "abcdefghijklmnož"),
        ("password", "abcd\tefghijklmnop"),
        ("sender_name", "Obec\r\nBcc: attacker@example.test"),
    ] {
        let mut input = google();
        input[field] = json!(value);
        assert_eq!(
            app.call("PUT", PATH, Some(input), true).await.status(),
            StatusCode::BAD_REQUEST,
            "accepted invalid {field}",
        );
    }
    let config = mail_settings::effective_config(&app.state).await.unwrap();
    assert_eq!(config.smtp_username.as_deref(), Some("sender@gmail.com"));
    assert_eq!(config.smtp_password.as_deref(), Some(APP_PASSWORD));
}

#[tokio::test]
async fn returning_to_environment_removes_saved_google_credentials() {
    let (app, _directory) = app().await;
    let mut config = (*app.state.config).clone();
    config.smtp_username = Some("environment-user".into());
    config.smtp_password = Some("environment-secret".into());
    let app = reconfigure(app, config.clone());
    let initial = body_json(app.call("GET", PATH, None, true).await).await;
    assert_eq!(initial["provider"], "environment");
    assert!(!initial.to_string().contains("environment-secret"));
    save(&app, google()).await;
    let reset = save(
        &app,
        json!({"provider": "environment", "current_password": ADMIN_PASSWORD}),
    )
    .await;
    assert_eq!(reset["provider"], "environment");
    assert_no_secrets(&reset);
    assert!(!reset.to_string().contains("environment-secret"));
    let effective = mail_settings::effective_config(&app.state).await.unwrap();
    assert_eq!(effective.smtp_host, config.smtp_host);
    assert_eq!(effective.smtp_port, config.smtp_port);
    assert_eq!(effective.smtp_tls, config.smtp_tls);
    assert_eq!(effective.smtp_username, config.smtp_username);
    assert_eq!(effective.smtp_password, config.smtp_password);
    assert_eq!(effective.email_from, config.email_from);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_settings")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0,
    );
    let mut without_password = google();
    without_password["password"] = json!("");
    assert_eq!(
        app.call("PUT", PATH, Some(without_password), true)
            .await
            .status(),
        StatusCode::BAD_REQUEST,
    );
}

#[tokio::test]
async fn missing_or_wrong_encryption_key_fails_closed_without_leaking_credentials() {
    let (app, _directory) = app().await;
    save(&app, google()).await;
    let key_path = &app.state.config.mail_settings_key_file;
    std::fs::remove_file(key_path).unwrap();
    assert!(mail_settings::effective_config(&app.state).await.is_err());
    let metadata = app.call("GET", PATH, None, true).await;
    assert_eq!(metadata.status(), StatusCode::OK);
    assert_no_secrets(&body_json(metadata).await);
    for (method, path, body) in [
        ("PUT", PATH, google()),
        (
            "POST",
            TEST_PATH,
            json!({"current_password": ADMIN_PASSWORD}),
        ),
    ] {
        let response = app.call(method, path, Some(body), true).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_no_secrets(&body_json(response).await);
    }
    assert!(
        !key_path.exists(),
        "an existing saved password must not create a replacement key"
    );
    std::fs::write(key_path, [0_u8; 32]).unwrap();
    assert!(mail_settings::effective_config(&app.state).await.is_err());
}

async fn smtp_capture() -> (u16, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"220 local.test ESMTP\r\n").await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut transcript = String::new();
        let mut data = false;
        loop {
            let mut line = String::new();
            if stream.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            transcript.push_str(&line);
            let response: &[u8] = if data {
                if line != ".\r\n" {
                    continue;
                }
                stream
                    .get_mut()
                    .write_all(b"250 received\r\n")
                    .await
                    .unwrap();
                break;
            } else if line.starts_with("EHLO") {
                b"250 local.test\r\n"
            } else if line.starts_with("MAIL FROM:") || line.starts_with("RCPT TO:") {
                b"250 accepted\r\n"
            } else if line == "DATA\r\n" {
                data = true;
                b"354 send message\r\n"
            } else {
                panic!("Unexpected SMTP command: {line}");
            };
            stream.get_mut().write_all(response).await.unwrap();
        }
        transcript
    });
    (port, received)
}

#[tokio::test]
async fn test_message_uses_configured_sender_and_only_the_authenticated_admin_recipient() {
    let (app, _directory) = app().await;
    let (port, received) = smtp_capture().await;
    let mut config = (*app.state.config).clone();
    config.smtp_port = port;
    let app = reconfigure(app, config);
    assert_eq!(
        app.call(
            "POST",
            TEST_PATH,
            Some(json!({"current_password": ADMIN_PASSWORD, "recipient": "another@example.test"})),
            true,
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    let response = app
        .call(
            "POST",
            TEST_PATH,
            Some(json!({"current_password": ADMIN_PASSWORD})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["recipient"], "admin@vysker.test");
    let transcript = tokio::time::timeout(std::time::Duration::from_secs(3), received)
        .await
        .unwrap()
        .unwrap();
    assert!(transcript.contains("MAIL FROM:<noreply@vysker.test>"));
    assert!(transcript.contains("RCPT TO:<admin@vysker.test>"));
    assert!(!transcript.contains("another@example.test"));
    assert!(!transcript.contains(ADMIN_PASSWORD));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0,
    );
}

#[tokio::test]
async fn smtp_test_failures_are_rate_limited() {
    let (app, _directory) = app().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.state.config).clone();
    config.smtp_port = listener.local_addr().unwrap().port();
    drop(listener);
    let app = reconfigure(app, config);
    for attempt in 0..4 {
        let response = app
            .call(
                "POST",
                TEST_PATH,
                Some(json!({"current_password": ADMIN_PASSWORD})),
                true,
            )
            .await;
        assert_eq!(
            response.status(),
            if attempt < 3 {
                StatusCode::BAD_GATEWAY
            } else {
                StatusCode::TOO_MANY_REQUESTS
            },
        );
        assert_no_secrets(&body_json(response).await);
    }
}

#[tokio::test]
async fn running_worker_refreshes_settings_and_leaves_mail_unclaimed_when_key_is_missing() {
    let (app, _directory) = app().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.state.config).clone();
    config.smtp_port = listener.local_addr().unwrap().port();
    drop(listener);
    let app = reconfigure(app, config);
    let mut worker = mail::Worker::new(&app.state).unwrap();
    save(&app, google()).await;
    std::fs::remove_file(&app.state.config.mail_settings_key_file).unwrap();
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "queued@example.test", now)
        .await
        .unwrap();
    assert!(worker.deliver_one(&app.state, now).await.is_err());
    let untouched: (i64, i64, Option<i64>) =
        sqlx::query_as("SELECT attempts,locked_until,sent_at FROM mail_queue")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(untouched, (0, 0, None));
    save(
        &app,
        json!({"provider": "environment", "current_password": ADMIN_PASSWORD}),
    )
    .await;
    assert!(worker.deliver_one(&app.state, now).await.unwrap());
    let retried: (i64, i64, Option<i64>) =
        sqlx::query_as("SELECT attempts,locked_until,sent_at FROM mail_queue")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(retried, (1, 0, None));
}

#[tokio::test]
async fn staging_capture_ignores_saved_google_and_captures_every_delivery_path() {
    let (mut app, _directory) = app().await;
    save(&app, google()).await;
    std::fs::remove_file(&app.state.config.mail_settings_key_file).unwrap();

    for path in ["test", "subscription", "recovery"] {
        let (port, received) = smtp_capture().await;
        let mut config = (*app.state.config).clone();
        config.smtp_capture_only = true;
        config.smtp_port = port;
        app = reconfigure(app, config);
        let effective = mail_settings::effective_config(&app.state).await.unwrap();
        assert_eq!(effective.smtp_host, "127.0.0.1");
        assert_eq!(effective.smtp_port, port);
        assert!(effective.smtp_username.is_none());
        assert!(effective.smtp_password.is_none());
        let settings = body_json(app.call("GET", PATH, None, true).await).await;
        assert_eq!(settings["capture_only"], true);
        assert_eq!(settings["provider"], "environment");
        assert_eq!(settings["smtp_port"], port);
        assert!(!settings.to_string().contains("sender@gmail.com"));
        // Neither replacing a provider nor returning to server configuration
        // can change transport policy from the administration.
        for input in [
            google(),
            json!({"provider": "environment", "current_password": ADMIN_PASSWORD}),
        ] {
            assert_eq!(
                app.call("PUT", PATH, Some(input), true).await.status(),
                StatusCode::BAD_REQUEST
            );
        }
        let now = OffsetDateTime::now_utc();
        match path {
            "test" => {
                assert_eq!(
                    app.call(
                        "POST",
                        TEST_PATH,
                        Some(json!({"current_password": ADMIN_PASSWORD})),
                        true
                    )
                    .await
                    .status(),
                    StatusCode::OK
                );
            }
            "subscription" => {
                common::request_subscription(&app.state, "captured@example.test", now)
                    .await
                    .unwrap();
                let mut worker = mail::Worker::new(&app.state).unwrap();
                assert!(worker.deliver_one(&app.state, now).await.unwrap());
                assert!(
                    sqlx::query_scalar::<_, bool>("SELECT sent_at IS NOT NULL FROM mail_queue")
                        .fetch_one(&app.state.pool)
                        .await
                        .unwrap()
                );
            }
            "recovery" => {
                assert_eq!(
                    app.call(
                        "POST",
                        "/api/v1/admin/password-recovery",
                        Some(json!({"email": "admin@vysker.test"})),
                        false
                    )
                    .await
                    .status(),
                    StatusCode::ACCEPTED
                );
                let mut worker = mail::Worker::new(&app.state).unwrap();
                assert!(
                    worker
                        .deliver_one(&app.state, OffsetDateTime::now_utc())
                        .await
                        .unwrap()
                );
                assert!(
                    sqlx::query_scalar::<_, bool>("SELECT sent_at IS NOT NULL FROM recovery_mail")
                        .fetch_one(&app.state.pool)
                        .await
                        .unwrap()
                );
            }
            _ => unreachable!(),
        }
        let transcript = tokio::time::timeout(std::time::Duration::from_secs(3), received)
            .await
            .unwrap()
            .unwrap();
        assert!(transcript.contains("MAIL FROM:<noreply@vysker.test>"));
        let recipient = if path == "subscription" {
            "captured@example.test"
        } else {
            "admin@vysker.test"
        };
        assert!(transcript.contains(&format!("RCPT TO:<{recipient}>")));
        assert!(!transcript.contains("AUTH"));
        assert!(!transcript.contains(APP_PASSWORD));
    }
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT username FROM mail_settings WHERE id=1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        "sender@gmail.com"
    );
    assert!(!app.state.config.mail_settings_key_file.exists());
}

#[tokio::test]
async fn staging_capture_rejects_external_or_authenticated_transports() {
    let (app, _directory) = app().await;
    let mut config = (*app.state.config).clone();
    config.smtp_capture_only = true;
    config.smtp_host = "mailpit".into();
    assert!(mail::transport(&Backend::new(app.state.pool.clone(), config.clone())).is_ok());
    for (host, port, tls, username, password, production) in [
        ("smtp.gmail.com", 1025, "none", None, None, false),
        ("192.0.2.1", 1025, "none", None, None, false),
        ("mailpit", 25, "none", None, None, false),
        ("mailpit", 1025, "starttls", None, None, false),
        ("mailpit", 1025, "none", Some("user"), Some("secret"), false),
        ("mailpit", 1025, "none", None, Some("secret"), false),
        ("mailpit", 1025, "none", None, None, true),
    ] {
        let mut invalid = config.clone();
        invalid.smtp_host = host.into();
        invalid.smtp_port = port;
        invalid.smtp_tls = tls.into();
        invalid.smtp_username = username.map(str::to_owned);
        invalid.smtp_password = password.map(str::to_owned);
        invalid.production = production;
        let state = Backend::new(app.state.pool.clone(), invalid);
        assert!(mail::transport(&state).is_err());
        assert!(mail_settings::effective_config(&state).await.is_err());
    }
}
