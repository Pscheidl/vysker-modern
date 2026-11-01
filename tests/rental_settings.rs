#![cfg(feature = "ssr")]
mod common;
#[path = "rental_v2_support/mod.rs"]
mod support;
use axum::{body::Body, http::Request, response::Response};
use common::{App, body_json};
use futures_util::StreamExt;
use obecni_web::backend::{auth, rental_settings, rentals};
use serde_json::{Value, json};
use std::time::Duration;
use time::OffsetDateTime;
use tower::ServiceExt;

async fn settings(app: &App) -> Value {
    body_json(
        app.call("GET", "/api/v1/admin/rental-settings", None, true)
            .await,
    )
    .await
}
async fn set_enabled(app: &App, enabled: bool) -> Response {
    let current = settings(app).await;
    app.call(
        "PUT",
        "/api/v1/admin/rental-settings",
        Some(json!({"enabled":enabled,"expected_version":current["version"]})),
        true,
    )
    .await
}
fn asset_input() -> Value {
    json!({"name":"Sál","description":"Test","rates":{"standard":{"hourly_price":100,"weekend_day_price":500},"nonprofit":{"hourly_price":0,"weekend_day_price":0}},"notification_email":"office@example.test","require_meter":true,"active":true})
}
fn request(app: &App, method: &str, path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", &app.cookie)
        .header("x-csrf-token", &app.csrf)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn fresh_install_is_off_and_all_business_routes_reject_before_reading_the_body() {
    let app = App::with_rentals(false).await;
    assert_eq!(settings(&app).await, json!({"enabled":false,"version":1}));
    assert!(!rental_settings::enabled(&app.state.pool).await.unwrap());
    for (method, path) in [
        ("GET", "/api/v1/rental-assets"),
        ("GET", "/api/v1/rental-assets/1/availability"),
        ("POST", "/api/v1/rentals"),
        ("GET", "/api/v1/rental-access/unknown"),
        ("POST", "/api/v1/rental-access/unknown/photos"),
        ("GET", "/api/v1/rental-access/unknown/photos/before"),
        ("GET", "/api/v1/admin/rental-assets"),
        ("POST", "/api/v1/admin/rental-assets"),
        ("PUT", "/api/v1/admin/rental-assets/1"),
        ("GET", "/api/v1/admin/rental-assets/1/blocks"),
        ("POST", "/api/v1/admin/rental-assets/1/blocks"),
        ("DELETE", "/api/v1/admin/rental-assets/1/blocks/1"),
        ("GET", "/api/v1/admin/rentals"),
        ("POST", "/api/v1/admin/rentals"),
        ("POST", "/api/v1/admin/rentals/preview"),
        ("GET", "/api/v1/admin/rentals/1"),
        ("POST", "/api/v1/admin/rentals/1/identity"),
        ("GET", "/api/v1/admin/rentals/1/photos/before"),
        ("POST", "/api/v1/admin/rentals/1/approve"),
        ("POST", "/api/v1/admin/rentals/1/reject"),
        ("POST", "/api/v1/admin/rentals/1/invoice"),
        ("POST", "/api/v1/admin/rentals/1/invoice-with-attachment"),
        ("POST", "/api/v1/admin/rentals/1/paid"),
        ("POST", "/api/v1/admin/rentals/1/cancel"),
        ("POST", "/api/v1/admin/rental-series/1/cancel"),
        ("GET", "/api/v1/admin/rental-payment-settings"),
        ("PUT", "/api/v1/admin/rental-payment-settings"),
    ] {
        let stream = futures_util::stream::once(async {
            panic!("Disabled rental API must not poll the request body");
            #[allow(unreachable_code)]
            Ok::<_, std::convert::Infallible>(axum::body::Bytes::new())
        });
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("cookie", &app.cookie)
                    .header("x-csrf-token", &app.csrf)
                    .header("content-type", "multipart/form-data; boundary=boundary")
                    .body(Body::from_stream(stream))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 503, "{method} {path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            body_json(response).await,
            json!({"error":"Rezervační systém je vypnutý."})
        );
    }
    assert_eq!(
        app.call("GET", "/api/v1/admin/rental-settings", None, false)
            .await
            .status(),
        401
    );
    assert_eq!(
        app.call("GET", "/api/v1/health", None, false)
            .await
            .status(),
        200
    );
    let rows:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM rentals),(SELECT count(*) FROM rental_assets),(SELECT count(*) FROM rental_mail_queue)").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(rows, (0, 0, 0));
}

#[tokio::test]
async fn only_current_rental_managers_can_toggle_with_csrf_version_and_audit() {
    let app = App::with_rentals(false).await;
    let password = "Read-only-rental-settings-password-123";
    let admin = auth::create_admin(&app.state, "basic@example.test", password.into())
        .await
        .unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"basic@example.test","password":password})),
            false,
        )
        .await;
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = body_json(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let input = json!({"enabled":true,"expected_version":1});
    let call = |method: &str, token: &str| {
        Request::builder()
            .method(method)
            .uri("/api/v1/admin/rental-settings")
            .header("cookie", &cookie)
            .header("x-csrf-token", token)
            .header("content-type", "application/json")
            .body(Body::from(input.to_string()))
            .unwrap()
    };
    assert_eq!(
        app.router
            .clone()
            .oneshot(call("GET", &csrf))
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        app.router
            .clone()
            .oneshot(call("PUT", &csrf))
            .await
            .unwrap()
            .status(),
        403
    );
    sqlx::query("UPDATE administrators SET can_view_identity=TRUE WHERE id=$1")
        .bind(admin)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.router
            .clone()
            .oneshot(call("PUT", "invalid"))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        app.router
            .clone()
            .oneshot(call("PUT", &csrf))
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(settings(&app).await, json!({"enabled":true,"version":2}));
    assert_eq!(
        app.call(
            "PUT",
            "/api/v1/admin/rental-settings",
            Some(json!({"enabled":false,"expected_version":1})),
            true
        )
        .await
        .status(),
        409
    );
    assert_eq!(set_enabled(&app, true).await.status(), 200);
    assert_eq!(settings(&app).await["version"], 2);
    sqlx::query("UPDATE administrators SET can_view_identity=FALSE WHERE id=$1")
        .bind(admin)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.router
            .clone()
            .oneshot(call("PUT", &csrf))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(set_enabled(&app, false).await.status(), 200);
    let log: Vec<(i64, String)> = sqlx::query_as(
        "SELECT actor_id,operation FROM audit_log WHERE entity_type='rental_settings' ORDER BY id",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        log,
        vec![(admin, "enabled".into()), (app.admin_id, "disabled".into())]
    );
    assert_eq!(settings(&app).await, json!({"enabled":false,"version":3}));
}

#[tokio::test]
async fn a_write_that_passed_the_initial_gate_rechecks_after_waiting_for_its_lock() {
    let app = App::new().await;
    let mut lock = obecni_web::db::begin_write(&app.state.pool).await.unwrap();
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let router = app.router.clone();
    let input = request(&app, "POST", "/api/v1/admin/rental-assets", asset_input());
    let pending = tokio::spawn(async move { router.oneshot(input).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(holder)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sqlx::query("UPDATE rental_settings SET enabled=FALSE,version=version+1 WHERE id=1")
        .execute(&mut *lock)
        .await
        .unwrap();
    lock.commit().await.unwrap();
    assert_eq!(pending.await.unwrap().status(), 503);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rental_assets")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn a_slow_public_upload_cannot_commit_after_the_system_is_disabled() {
    let app = App::new().await;
    let asset = support::asset(&app, true).await;
    let day = support::monday().to_string();
    let mut image = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(3, 3)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    let mut body = Vec::new();
    for (key, value) in [
        ("asset_id", asset.to_string()),
        ("event_type", "standard".into()),
        ("start_date", day.clone()),
        ("end_date", day),
        ("start_time", "18:00".into()),
        ("end_time", "19:00".into()),
        ("name", "Test".into()),
        ("email", "tenant@example.test".into()),
        ("phone", "123456789".into()),
        ("address", "Vyskeř 1".into()),
        ("consent", "true".into()),
    ] {
        body.extend_from_slice(
            format!(
                "--boundary\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"identity\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n");
    body.extend_from_slice(image.get_ref());
    body.extend_from_slice(b"\r\n--boundary--\r\n");
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    let tail = body.split_off(body.len() - 8);
    let stream = futures_util::stream::once(async move {
        let _ = started.send(());
        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(body))
    })
    .chain(futures_util::stream::once(async move {
        let _ = resume_rx.await;
        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(tail))
    }));
    let router = app.router.clone();
    let pending = tokio::spawn(
        router.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/rentals")
                .header("content-type", "multipart/form-data; boundary=boundary")
                .body(Body::from_stream(stream))
                .unwrap(),
        ),
    );
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(set_enabled(&app, false).await.status(), 200);
    resume.send(()).unwrap();
    assert_eq!(pending.await.unwrap().unwrap().status(), 503);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rentals),(SELECT count(*) FROM rental_mail_queue)",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
}

#[tokio::test]
async fn disabled_workers_preserve_pending_mail_and_pause_reminders_and_automatic_invoices() {
    let app = App::new().await;
    let asset = support::asset(&app, true).await;
    let day = support::monday();
    let mut input = support::admin_input(asset, day, Some("18:00"), Some("19:00"));
    input["rental_price"] = json!(0);
    input["recurrence_interval_days"] = json!(7);
    input["recurrence_until"] = json!(day.to_string());
    let automatic = support::create(&app, input).await["rentals"][0]["id"]
        .as_i64()
        .unwrap();
    let mut input = support::admin_input(asset, day, Some("20:00"), Some("21:00"));
    input["require_meter"] = json!(true);
    let meter = support::create(&app, input).await["rentals"][0]["id"]
        .as_i64()
        .unwrap();
    let upcoming = support::create(
        &app,
        support::admin_input(asset, day, Some("22:00"), Some("23:00")),
    )
    .await["rentals"][0]["id"]
        .as_i64()
        .unwrap();
    sqlx::query(
        "UPDATE rentals SET start_date=CURRENT_DATE-1,end_date=CURRENT_DATE-1 WHERE id IN ($1,$2)",
    )
    .bind(automatic)
    .bind(meter)
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM rental_mail_queue")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,'tenant@example.test','Test','Outdated approval','approval',0,0)").bind(automatic).execute(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,'tenant@example.test','Test','Keep me','approval',0,0)").bind(upcoming).execute(&app.state.pool).await.unwrap();
    assert_eq!(set_enabled(&app, false).await.status(), 200);
    let now = OffsetDateTime::now_utc();
    rentals::maintenance(&app.state, now).await.unwrap();
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(9)
        .build();
    assert!(!rentals::deliver_one(&app.state, &smtp, now).await.unwrap());
    let unchanged: (i64, i64, String) =
        sqlx::query_as("SELECT count(*),sum(attempts)::bigint,min(body) FROM rental_mail_queue")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(unchanged, (2, 0, "Keep me".into()));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rentals WHERE status='approved' AND last_reminder_date IS NULL"
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap(),
        3
    );
    assert_eq!(set_enabled(&app, true).await.status(), 200);
    rentals::maintenance(&app.state, now).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM rentals WHERE id=$1")
            .bind(automatic)
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        "invoiced"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rental_mail_queue WHERE purpose IN ('reminder','reminder_admin')"
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap(),
        2
    );
    // Resuming revalidates queued mail. The now-invoiced term's old approval is
    // discarded, while the upcoming rental's queued message survives.
    assert!(rentals::deliver_one(&app.state, &smtp, now).await.unwrap());
    let obsolete: (bool, i64) = sqlx::query_as("SELECT cancelled_at IS NOT NULL AND sent_at IS NULL AND body IS NULL,attempts FROM rental_mail_queue WHERE rental_id=$1 AND purpose='approval'")
        .bind(automatic).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(obsolete, (true, 0));
    let (port, capture) = support::smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    assert!(rentals::deliver_one(&app.state, &smtp, now).await.unwrap());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), capture)
            .await
            .unwrap()
            .unwrap()
            .contains("Keep me")
    );
}

#[tokio::test]
async fn disabling_waits_for_an_already_running_smtp_send_and_prevents_the_next_one() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let id = support::create(
        &app,
        support::admin_input(asset, support::monday(), Some("18:00"), Some("19:00")),
    )
    .await["rentals"][0]["id"]
        .as_i64()
        .unwrap();
    sqlx::query("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,'tenant@example.test','Second','Must wait','approval',0,0)").bind(id).execute(&app.state.pool).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        writer.write_all(b"220 localhost SMTP\r\n").await.unwrap();
        let mut data = false;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            if data {
                if line == ".\r\n" {
                    let _ = started.send(());
                    let _ = resume_rx.await;
                    writer.write_all(b"250 accepted\r\n").await.unwrap();
                    break;
                }
            } else if line.starts_with("DATA") {
                data = true;
                writer.write_all(b"354 send\r\n").await.unwrap();
            } else {
                writer.write_all(b"250 localhost\r\n").await.unwrap();
            }
        }
    });
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    let worker = app.state.clone();
    let delivery = tokio::spawn(async move {
        rentals::deliver_one(
            &worker,
            &smtp,
            OffsetDateTime::now_utc() + time::Duration::seconds(1),
        )
        .await
        .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    let version = settings(&app).await["version"].clone();
    let router = app.router.clone();
    let input = request(
        &app,
        "PUT",
        "/api/v1/admin/rental-settings",
        json!({"enabled":false,"expected_version":version}),
    );
    let mut disabling = tokio::spawn(router.oneshot(input));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut disabling)
            .await
            .is_err()
    );
    resume.send(()).unwrap();
    assert!(delivery.await.unwrap());
    server.await.unwrap();
    assert_eq!(disabling.await.unwrap().unwrap().status(), 200);
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(9)
        .build();
    assert!(
        !rentals::deliver_one(
            &app.state,
            &smtp,
            OffsetDateTime::now_utc() + time::Duration::seconds(1)
        )
        .await
        .unwrap()
    );
    let preserved: (i64, i64) = sqlx::query_as(
        "SELECT count(*),sum(attempts)::bigint FROM rental_mail_queue WHERE sent_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(preserved, (1, 0));
}
