#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, body_json};
use image::{DynamicImage, ImageFormat};
use obecni_web::backend::{rentals, today};
use serde_json::json;
use std::io::Cursor;
use time::{Date, Duration, OffsetDateTime, Weekday};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tower::ServiceExt;

fn photo() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(3, 2)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn monday() -> Date {
    let mut day = today(OffsetDateTime::now_utc()) + Duration::days(7);
    while day.weekday() != Weekday::Monday {
        day += Duration::days(1);
    }
    day
}

async fn asset(app: &App) -> i64 {
    let res = app
        .call(
            "POST",
            "/api/v1/admin/rental-assets",
            Some(json!({
                "name":"Obecní sál", "description":"Sál", "rates":{"nonprofit":{"hourly_price":0,"weekend_day_price":0},"standard":{"hourly_price":100,"weekend_day_price":150}},
                "notification_email":"office@example.test", "require_meter":true, "active":true
            })),
            true,
        )
        .await;
    assert!(res.status().is_success(), "{}", res.status());
    body_json(res).await["id"].as_i64().unwrap()
}

fn multipart(fields: &[(&str, &str)], file_name: &str, file: &[u8]) -> Vec<u8> {
    multipart_files(fields, &[(file_name, file)])
}

fn multipart_files(fields: &[(&str, &str)], files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--boundary\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    for (file_name, file) in files {
        body.extend_from_slice(format!("--boundary\r\nContent-Disposition: form-data; name=\"{file_name}\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
        body.extend_from_slice(file);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--boundary--\r\n");
    body
}

async fn request(
    app: &App,
    asset_id: i64,
    start: Date,
    end: Date,
    consent: &str,
    file: &[u8],
) -> Response {
    let asset_id = asset_id.to_string();
    let hourly = start == end && !matches!(start.weekday(), Weekday::Saturday | Weekday::Sunday);
    let start = start.to_string();
    let end = end.to_string();
    let mut fields = vec![
        ("asset_id", asset_id.as_str()),
        ("event_type", "standard"),
        ("start_date", start.as_str()),
        ("end_date", end.as_str()),
        ("name", "Jan Novák"),
        ("email", "jan@example.test"),
        ("phone", "+420 123 456 789"),
        ("address", "Vyskeř 1"),
        ("consent", consent),
    ];
    if hourly {
        fields.extend([("start_time", "18:00"), ("end_time", "19:00")]);
    }
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/rentals")
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(multipart(&fields, "identity", file)))
        .unwrap();
    app.router.clone().oneshot(request).await.unwrap()
}

async fn upload(app: &App, token: &str, kind: &str, file: &[u8]) -> Response {
    upload_fields(app, token, &[("kind", kind)], &[("photo", file)]).await
}

async fn upload_fields(
    app: &App,
    token: &str,
    fields: &[(&str, &str)],
    files: &[(&str, &[u8])],
) -> Response {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/rental-access/{token}/photos"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(multipart_files(fields, files)))
        .unwrap();
    app.router.clone().oneshot(request).await.unwrap()
}

async fn approved_rental(app: &App) -> (i64, String) {
    let asset_id = asset(app).await;
    let day = monday();
    let result = request(app, asset_id, day, day, "true", &photo()).await;
    assert_eq!(result.status(), StatusCode::CREATED);
    let id = body_json(result).await["id"].as_i64().unwrap();
    assert!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/approve"),
            None,
            true
        )
        .await
        .status()
        .is_success()
    );
    (id, approval_token(app, id).await)
}

async fn approval_token(app: &App, id: i64) -> String {
    let body: String = sqlx::query_scalar(
        "SELECT body FROM rental_mail_queue WHERE rental_id=$1 AND purpose='approval'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    body.split("/pronajmy/meridlo/")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

async fn smtp_capture() -> (u16, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        writer.write_all(b"220 localhost SMTP\r\n").await.unwrap();
        let mut message = String::new();
        let mut data = false;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            if data {
                if line == ".\r\n" {
                    writer.write_all(b"250 message accepted\r\n").await.unwrap();
                    break;
                }
                message.push_str(&line);
            } else if line.starts_with("DATA") {
                data = true;
                writer.write_all(b"354 send message\r\n").await.unwrap();
            } else {
                writer.write_all(b"250 localhost\r\n").await.unwrap();
            }
        }
        message
    });
    (port, task)
}

fn decode_base64(input: &str) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("Invalid base64 in attachment"),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    output
}

#[tokio::test]
async fn date_validation_conflicts_and_simultaneous_requests() {
    let app = App::new().await;
    let id = asset(&app).await;
    let monday = monday();
    let friday = monday + Duration::days(5);
    let bytes = photo();
    assert_eq!(
        request(
            &app,
            id,
            monday,
            monday + Duration::days(28),
            "true",
            &bytes
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&app, id, monday, monday, "false", &bytes)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&app, id, monday, monday, "true", b"not an image")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let (a, b) = tokio::join!(
        request(&app, id, friday, friday + Duration::days(1), "true", &bytes),
        request(&app, id, friday, friday + Duration::days(1), "true", &bytes)
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    let res = app
        .call(
            "GET",
            &format!("/api/v1/rental-assets/{id}/availability"),
            None,
            false,
        )
        .await;
    let availability = body_json(res).await;
    assert_eq!(availability.as_array().unwrap().len(), 1);
    assert_eq!(availability[0]["start_date"], friday.to_string());
    assert_eq!(
        availability[0]["end_date"],
        (friday + Duration::days(1)).to_string()
    );
    assert_eq!(availability[0]["status"], "pending");
    assert!(availability[0]["start_time"].is_null());
    assert!(availability[0]["end_time"].is_null());
    assert_eq!(availability[0].as_object().unwrap().len(), 5);
    let charged: i64 = sqlx::query_scalar("SELECT rental_price FROM rentals")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(charged, 300);
    let rental_id: i64 = sqlx::query_scalar("SELECT id FROM rentals")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{rental_id}/approve"),
            None,
            true
        )
        .await
        .status()
        .is_success()
    );
    let confirmed = body_json(
        app.call(
            "GET",
            &format!("/api/v1/rental-assets/{id}/availability"),
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(confirmed[0]["status"], "booked");
    let rejected = request(&app, id, monday, monday, "true", &bytes).await;
    assert_eq!(rejected.status(), StatusCode::CREATED);
    let rejected_id = body_json(rejected).await["id"].as_i64().unwrap();
    assert!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{rejected_id}/reject"),
            None,
            true
        )
        .await
        .status()
        .is_success()
    );
    let rejection: String = sqlx::query_scalar(
        "SELECT body FROM rental_mail_queue WHERE rental_id=$1 AND purpose='rejected'",
    )
    .bind(rejected_id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(rejection.contains("nebyla schválena. Omlouváme se."));
    let availability = body_json(
        app.call(
            "GET",
            &format!("/api/v1/rental-assets/{id}/availability"),
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(availability.as_array().unwrap().len(), 1);
    assert_eq!(availability[0]["status"], "booked");
}

#[tokio::test]
async fn friday_is_hourly_and_saturday_sunday_are_independent_days() {
    let app = App::new().await;
    let id = asset(&app).await;
    let friday = monday() + Duration::days(4);
    let bytes = photo();
    for (offset, price) in [(0, 100), (1, 150), (2, 150)] {
        let day = friday + Duration::days(offset);
        let response = request(&app, id, day, day, "true", &bytes).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let rental_id = body_json(response).await["id"].as_i64().unwrap();
        let charged: i64 = sqlx::query_scalar("SELECT rental_price FROM rentals WHERE id=$1")
            .bind(rental_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(charged, price);
    }
}

#[tokio::test]
async fn approval_photos_invoice_and_private_identity_lifecycle() {
    let app = App::new().await;
    let asset_id = asset(&app).await;
    let monday = monday();
    let bytes = photo();
    let result = request(&app, asset_id, monday, monday, "true", &bytes).await;
    assert_eq!(result.status(), StatusCode::CREATED);
    let id = body_json(result).await["id"].as_i64().unwrap();
    let identity = format!("/api/v1/admin/rentals/{id}/identity");
    assert_eq!(
        app.call("GET", &identity, None, true).await.status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/rentals/{id}/photos/identity"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "POST",
            &identity,
            Some(json!({"password":"Testovaci-dlouhe-heslo-123456!"})),
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let no_csrf = Request::builder()
        .method("POST")
        .uri(&identity)
        .header("cookie", &app.cookie)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"password":"Testovaci-dlouhe-heslo-123456!"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.router.clone().oneshot(no_csrf).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.call("POST", &identity, Some(json!({"password":"wrong"})), true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = app
        .call(
            "POST",
            &identity,
            Some(json!({"password":"Testovaci-dlouhe-heslo-123456!"})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    // A successful attempt does not unlock subsequent requests.
    assert_eq!(
        app.call("POST", &identity, Some(json!({"password":"wrong"})), true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let approve = format!("/api/v1/admin/rentals/{id}/approve");
    assert_eq!(
        app.call("POST", &approve, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        app.call("POST", &approve, None, true)
            .await
            .status()
            .is_success()
    );
    let token = approval_token(&app, id).await;
    // Reject an unknown token before parsing or decoding a supplied image.
    assert_eq!(
        upload(&app, &"a".repeat(64), "before", b"not an image")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/rental-access/{}", "a".repeat(64)),
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
            &format!("/api/v1/rental-access/{token}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    let invoice = format!("/api/v1/admin/rentals/{id}/invoice");
    let input = json!({"energy_kwh":"12.5","energy_cost":125,"extra_cost":0,"account_iban":"CZ6508000000192000145399","due_days":14});
    assert_eq!(
        app.call("POST", &invoice, Some(input.clone()), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE rentals SET start_date=CURRENT_DATE-1,end_date=CURRENT_DATE-1 WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("POST", &invoice, Some(input.clone()), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        upload(&app, &token, "before", &bytes).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        upload(&app, &token, "after", &bytes).await.status(),
        StatusCode::OK
    );
    let response = app.call("POST", &invoice, Some(input), true).await;
    assert!(response.status().is_success());
    let invoiced = body_json(response).await;
    assert_eq!(invoiced["invoice_mail_status"], "pending");
    assert!(invoiced["invoice_sent_at"].is_null());
    let row: (String, i64) = sqlx::query_as("SELECT status,total_price FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(row, ("invoiced".to_owned(), 225));
    let mail: String = sqlx::query_scalar(
        "SELECT body FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(mail.contains("225 Kč"));
    // Deliver the actual invoice through SMTP and decode its attachment.
    sqlx::query("UPDATE rental_mail_queue SET sent_at=1,body=NULL WHERE rental_id=$1 AND purpose<>'invoice'")
        .bind(id).execute(&app.state.pool).await.unwrap();
    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_port = unused.local_addr().unwrap().port();
    drop(unused);
    let failing =
        lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(closed_port)
            .timeout(Some(std::time::Duration::from_millis(200)))
            .build();
    let attempt_time = OffsetDateTime::now_utc() + Duration::seconds(1);
    assert!(
        rentals::deliver_one(&app.state, &failing, attempt_time)
            .await
            .unwrap()
    );
    let retrying = body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
            .await,
    )
    .await;
    assert_eq!(retrying["invoice_mail_status"], "retrying");
    assert!(retrying["invoice_sent_at"].is_null());
    let (port, capture) = smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    assert!(
        rentals::deliver_one(&app.state, &smtp, attempt_time + Duration::seconds(61))
            .await
            .unwrap()
    );
    let message = tokio::time::timeout(std::time::Duration::from_secs(3), capture)
        .await
        .unwrap()
        .unwrap();
    assert!(message.contains("To: jan@example.test"));
    assert!(message.contains("image/png"));
    assert!(message.contains("qr-platba.png"));
    let part = message
        .split("Content-Type: image/png")
        .nth(1)
        .expect("PNG attachment");
    let base64 = part
        .split("\r\n\r\n")
        .nth(1)
        .expect("attachment body")
        .split("\r\n--")
        .next()
        .unwrap();
    let png = decode_base64(base64);
    assert_eq!(image::guess_format(&png).unwrap(), ImageFormat::Png);
    assert!(image::load_from_memory(&png).unwrap().width() > 0);
    let sent: bool = sqlx::query_scalar("SELECT sent_at IS NOT NULL AND body IS NULL FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert!(sent);
    let delivered = body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
            .await,
    )
    .await;
    assert_eq!(delivered["invoice_mail_status"], "sent");
    assert!(delivered["invoice_sent_at"].as_str().is_some());
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/rental-access/{token}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "POST",
            &identity,
            Some(json!({"password":"Testovaci-dlouhe-heslo-123456!"})),
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/paid"),
            None,
            true
        )
        .await
        .status()
        .is_success()
    );
    assert_eq!(
        app.call(
            "POST",
            &identity,
            Some(json!({"password":"Testovaci-dlouhe-heslo-123456!"})),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let stored: bool = sqlx::query_scalar("SELECT identity IS NULL FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(stored);
}

#[tokio::test]
async fn reminders_are_daily_go_to_both_recipients_and_stop_when_uploaded() {
    let app = App::new().await;
    let asset_id = asset(&app).await;
    let day = monday();
    let bytes = photo();
    let result = request(&app, asset_id, day, day, "true", &bytes).await;
    let id = body_json(result).await["id"].as_i64().unwrap();
    app.call(
        "POST",
        &format!("/api/v1/admin/rentals/{id}/approve"),
        None,
        true,
    )
    .await;
    let token = approval_token(&app, id).await;
    sqlx::query("UPDATE rentals SET start_date=CURRENT_DATE-1,end_date=CURRENT_DATE-1 WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let now = OffsetDateTime::now_utc();
    rentals::maintenance(&app.state, now).await.unwrap();
    rentals::maintenance(&app.state, now).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin')").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(count, 2);
    let recipients: Vec<String> = sqlx::query_scalar("SELECT recipient FROM rental_mail_queue WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin') ORDER BY recipient").bind(id).fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(recipients, vec!["jan@example.test", "office@example.test"]);
    rentals::maintenance(&app.state, now + Duration::days(1))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin')").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(count, 4);
    // Unsent reminders from yesterday are superseded, so an SMTP outage
    // cannot deliver several days of reminders at once when it recovers.
    let superseded: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin') AND cancelled_at IS NOT NULL AND body IS NULL").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(superseded, 2);
    assert_eq!(
        upload(&app, &token, "before", &bytes).await.status(),
        StatusCode::OK
    );
    let still_pending: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin') AND cancelled_at IS NULL AND body IS NOT NULL").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(still_pending, 2);
    assert_eq!(
        upload(&app, &token, "after", &bytes).await.status(),
        StatusCode::OK
    );
    let cancelled: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin') AND cancelled_at IS NOT NULL AND body IS NULL").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(cancelled, 4);
    rentals::maintenance(&app.state, now + Duration::days(2))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin')").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(count, 4);
}

#[tokio::test]
async fn expired_pending_application_is_cancelled_and_cannot_be_decided_again() {
    let app = App::new().await;
    let asset_id = asset(&app).await;
    let day = monday();
    let response = request(&app, asset_id, day, day, "true", &photo()).await;
    let id = body_json(response).await["id"].as_i64().unwrap();
    sqlx::query("UPDATE rentals SET start_date=CURRENT_DATE-1,end_date=CURRENT_DATE-1 WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/approve"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/reject"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let status: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");
    let removed: bool = sqlx::query_scalar("SELECT identity IS NULL FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(removed);
}

#[tokio::test]
async fn zero_price_asset_without_meter_is_settled_without_iban_or_qr() {
    let app = App::new().await;
    let response = app
        .call(
            "POST",
            "/api/v1/admin/rental-assets",
            Some(json!({
                "name": "Park", "description": "Veřejný prostor", "rates":{"nonprofit":{"hourly_price":0,"weekend_day_price":0},"standard":{"hourly_price":0,"weekend_day_price":0}}, "notification_email": "office@example.test",
                "require_meter": false, "active": true
            })),
            true,
        )
        .await;
    let asset_id = body_json(response).await["id"].as_i64().unwrap();
    let day = monday();
    let response = request(&app, asset_id, day, day, "true", &photo()).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = body_json(response).await["id"].as_i64().unwrap();
    assert!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/approve"),
            None,
            true
        )
        .await
        .status()
        .is_success()
    );
    sqlx::query("UPDATE rentals SET start_date=CURRENT_DATE-1,end_date=CURRENT_DATE-1 WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let input = json!({"energy_kwh":"0", "energy_cost":0, "extra_cost":0, "account_iban":"", "due_days":14});
    let response = app
        .call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/invoice"),
            Some(input),
            true,
        )
        .await;
    assert!(response.status().is_success());
    let invoice = body_json(response).await;
    assert_eq!(invoice["total_price"], 0);
    let body: String = sqlx::query_scalar(
        "SELECT body FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(body.contains("není třeba nic platit"));
    assert!(!body.contains("IBAN"));
    sqlx::query("UPDATE rental_mail_queue SET sent_at=1,body=NULL WHERE rental_id=$1 AND purpose<>'invoice'")
        .bind(id).execute(&app.state.pool).await.unwrap();
    let (port, capture) = smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    assert!(
        rentals::deliver_one(
            &app.state,
            &smtp,
            OffsetDateTime::now_utc() + Duration::seconds(1)
        )
        .await
        .unwrap()
    );
    let message = tokio::time::timeout(std::time::Duration::from_secs(3), capture)
        .await
        .unwrap()
        .unwrap();
    assert!(message.contains("To: jan@example.test"));
    assert!(!message.contains("image/png"));
    assert!(!message.contains("qr-platba.png"));
    assert!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/paid"),
            None,
            true
        )
        .await
        .status()
        .is_success()
    );
}

#[tokio::test]
async fn blocks_can_be_added_removed_and_prevent_overlaps_and_races() {
    let app = App::new().await;
    let asset_id = asset(&app).await;
    let day = monday();
    let path = format!("/api/v1/admin/rental-assets/{asset_id}/blocks");
    let input = |first: Date, last: Date| json!({"start_date": first.to_string(), "end_date": last.to_string()});
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call("POST", &path, Some(input(day, day)), false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(input(day + Duration::days(1), day)),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let saturday = day + Duration::days(5);
    let block = app
        .call("POST", &path, Some(input(saturday, saturday)), true)
        .await;
    assert_eq!(block.status(), StatusCode::CREATED);
    let block_id = body_json(block).await["id"].as_i64().unwrap();
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(input(saturday, saturday + Duration::days(2))),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let availability = body_json(
        app.call(
            "GET",
            &format!("/api/v1/rental-assets/{asset_id}/availability"),
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(availability.as_array().unwrap().len(), 1);
    assert_eq!(
        availability[0],
        json!({"start_date":saturday.to_string(),"end_date":saturday.to_string(),"start_time":null,"end_time":null,"status":"booked"})
    );
    assert_eq!(
        request(
            &app,
            asset_id,
            day + Duration::days(5),
            day + Duration::days(6),
            "true",
            &photo()
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call("DELETE", &format!("{path}/{block_id}"), None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call("DELETE", &format!("{path}/{block_id}"), None, true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("DELETE", &format!("{path}/{block_id}"), None, true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, asset_id, day, day, "true", &photo())
            .await
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.call("POST", &path, Some(input(day, day)), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let simultaneous_day = day + Duration::days(1);
    let picture = photo();
    let (block, rental) = tokio::join!(
        app.call(
            "POST",
            &path,
            Some(input(simultaneous_day, simultaneous_day)),
            true
        ),
        request(
            &app,
            asset_id,
            simultaneous_day,
            simultaneous_day,
            "true",
            &picture
        )
    );
    let mut statuses = [block.status().as_u16(), rental.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE entity_type='rental_block' AND operation IN ('created','deleted')").fetch_one(&app.state.pool).await.unwrap();
    assert!(audit >= 2);
}

#[tokio::test]
async fn availability_returns_all_future_blocks_and_omits_expired() {
    let app = App::new().await;
    let asset_id = asset(&app).await;
    sqlx::query("INSERT INTO rental_blocks(asset_id,start_date,end_date) SELECT $1,DATE '2090-01-01' + n,DATE '2090-01-01' + n FROM generate_series(1,1005) AS n")
        .bind(asset_id).execute(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO rental_blocks(asset_id,start_date,end_date) VALUES ($1,DATE '2000-01-01',DATE '2000-01-02')")
        .bind(asset_id).execute(&app.state.pool).await.unwrap();
    let result = body_json(
        app.call(
            "GET",
            &format!("/api/v1/rental-assets/{asset_id}/availability"),
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(result.as_array().unwrap().len(), 1005);
    assert!(
        result
            .as_array()
            .unwrap()
            .iter()
            .all(|period| period["status"] == "booked")
    );
}

#[tokio::test]
async fn admin_rentals_include_all_closed_records_for_accurate_totals() {
    let app = App::new().await;
    let asset_id = asset(&app).await;
    sqlx::query("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter) SELECT $1,CURRENT_DATE,CURRENT_DATE,'Test','test@example.test','123456','Test',CASE WHEN n % 2=0 THEN 'paid' ELSE 'rejected' END,0,'now','now',FALSE FROM generate_series(1,1005) AS n")
        .bind(asset_id).execute(&app.state.pool).await.unwrap();
    let result = body_json(app.call("GET", "/api/v1/admin/rentals", None, true).await).await;
    assert_eq!(result.as_array().unwrap().len(), 1005);
    assert_eq!(
        result
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["status"] == "paid")
            .count(),
        502
    );
    assert_eq!(
        result
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["status"] == "rejected")
            .count(),
        503
    );
}

#[tokio::test]
async fn unknown_meter_token_is_rejected_without_reading_body() {
    use axum::body::Bytes;
    let app = App::new().await;
    let pending = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/rental-access/{}/photos", "a".repeat(64)))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from_stream(pending))
        .unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        app.router.clone().oneshot(request),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

fn meter_photo_with_time() -> Vec<u8> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode_image(&DynamicImage::new_rgb8(3, 2))
        .unwrap();
    let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0".to_vec();
    exif.extend_from_slice(&[1, 0, 0x69, 0x87, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0]);
    exif.extend_from_slice(&[1, 0, 0x03, 0x90, 2, 0, 20, 0, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0]);
    exif.extend_from_slice(b"2026:10:30 12:34:56\0");
    let mut result = vec![0xff, 0xd8, 0xff, 0xe1];
    result.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    result.extend_from_slice(&exif);
    result.extend_from_slice(&jpeg[2..]);
    result
}

#[tokio::test]
async fn meter_photos_use_one_link_for_staged_and_combined_uploads_and_private_previews() {
    let app = App::new().await;
    let (id, token) = approved_rental(&app).await;
    let path = format!("/api/v1/rental-access/{token}");
    let original = meter_photo_with_time();
    let plain = photo();
    let initial = body_json(app.call("GET", &path, None, false).await).await;
    assert_eq!(initial["before_uploaded"], false);
    assert_eq!(initial["after_uploaded"], false);
    assert!(initial["before_photo_taken_at"].is_null());

    let response = upload_fields(&app, &token, &[], &[("before", &original)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let first = body_json(response).await;
    assert_eq!(first["before_uploaded"], true);
    assert_eq!(first["after_uploaded"], false);
    assert_eq!(first["before_photo_taken_at"], "2026-10-30 12:34:56");
    assert_eq!(
        first,
        body_json(app.call("GET", &path, None, false).await).await
    );
    assert_eq!(
        app.call("GET", &format!("{path}/photos/after"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let preview = app
        .call("GET", &format!("{path}/photos/before"), None, false)
        .await;
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(preview.headers()["content-type"], "image/jpeg");
    assert_eq!(preview.headers()["cache-control"], "private, no-store");
    assert_eq!(preview.headers()["x-content-type-options"], "nosniff");
    assert_eq!(preview.headers()["content-disposition"], "inline");
    let stored_before = axum::body::to_bytes(preview.into_body(), 20 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        image::guess_format(&stored_before).unwrap(),
        ImageFormat::Jpeg
    );
    assert_eq!(image::load_from_memory(&stored_before).unwrap().width(), 3);
    assert!(
        exif::Reader::new()
            .read_from_container(&mut Cursor::new(&stored_before))
            .is_err()
    );

    let response = upload_fields(&app, &token, &[], &[("after", &plain)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let second = body_json(response).await;
    assert_eq!(second["before_uploaded"], true);
    assert_eq!(second["after_uploaded"], true);
    assert_eq!(second["before_photo_taken_at"], "2026-10-30 12:34:56");
    assert!(second["after_photo_taken_at"].is_null());
    let preview = app
        .call("GET", &format!("{path}/photos/before"), None, false)
        .await;
    assert_eq!(
        axum::body::to_bytes(preview.into_body(), 20 * 1024 * 1024)
            .await
            .unwrap(),
        stored_before
    );

    // The original link stays usable even after both photos have been received.
    let response = upload_fields(
        &app,
        &token,
        &[],
        &[("before", &plain), ("after", &original)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let both = body_json(response).await;
    assert_eq!(both["before_uploaded"], true);
    assert_eq!(both["after_uploaded"], true);
    assert!(both["before_photo_taken_at"].is_null());
    assert_eq!(both["after_photo_taken_at"], "2026-10-30 12:34:56");
    assert_eq!(
        both,
        body_json(app.call("GET", &path, None, false).await).await
    );
    let tokens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rental_access_tokens WHERE rental_id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(tokens, 1);
}

#[tokio::test]
async fn invalid_meter_uploads_never_partially_replace_saved_photos() {
    let app = App::new().await;
    let (id, token) = approved_rental(&app).await;
    let original = meter_photo_with_time();
    let replacement = photo();
    assert_eq!(
        upload_fields(
            &app,
            &token,
            &[],
            &[("before", &original), ("after", &original)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let saved: (Vec<u8>, Vec<u8>, Option<String>, Option<String>) = sqlx::query_as("SELECT before_photo,after_photo,before_photo_taken_at,after_photo_taken_at FROM rentals WHERE id=$1")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    let cases: Vec<(Vec<(&str, &str)>, Vec<(&str, &[u8])>)> = vec![
        (vec![], vec![]),
        (
            vec![],
            vec![("before", &replacement), ("after", b"not a photo")],
        ),
        (vec![], vec![("before", b""), ("after", &replacement)]),
        (
            vec![],
            vec![("before", &replacement), ("before", &replacement)],
        ),
        (
            vec![],
            vec![("after", &replacement), ("after", &replacement)],
        ),
        (vec![], vec![("identity", &replacement)]),
        (vec![("unknown", "x")], vec![("before", &replacement)]),
        (
            vec![("kind", "before")],
            vec![("photo", &replacement), ("after", &replacement)],
        ),
        (vec![("kind", "before")], vec![("before", &replacement)]),
        (
            vec![],
            vec![("photo", &replacement), ("before", &replacement)],
        ),
        (
            vec![("kind", "before"), ("kind", "after")],
            vec![("photo", &replacement)],
        ),
        (vec![("kind", "identity")], vec![("photo", &replacement)]),
        (vec![], vec![("photo", &replacement)]),
        (vec![("kind", "before")], vec![]),
    ];
    for (index, (fields, files)) in cases.into_iter().enumerate() {
        assert_eq!(
            upload_fields(&app, &token, &fields, &files).await.status(),
            StatusCode::BAD_REQUEST,
            "invalid case {index}"
        );
        let current: (Vec<u8>, Vec<u8>, Option<String>, Option<String>) = sqlx::query_as("SELECT before_photo,after_photo,before_photo_taken_at,after_photo_taken_at FROM rentals WHERE id=$1")
            .bind(id).fetch_one(&app.state.pool).await.unwrap();
        assert_eq!(current, saved, "invalid case {index} changed stored photos");
    }
    let oversized = vec![0u8; 10 * 1024 * 1024 + 1];
    assert_eq!(
        upload_fields(
            &app,
            &token,
            &[],
            &[("before", &replacement), ("after", &oversized)]
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let unchanged: (Vec<u8>, Vec<u8>, Option<String>, Option<String>) = sqlx::query_as("SELECT before_photo,after_photo,before_photo_taken_at,after_photo_taken_at FROM rentals WHERE id=$1")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(unchanged, saved);
}

#[tokio::test]
async fn meter_preview_is_scoped_to_its_token_status_and_feature_setting() {
    let app = App::new().await;
    let (id, token) = approved_rental(&app).await;
    let (_, other_token) = approved_rental(&app).await;
    let bytes = photo();
    assert_eq!(
        upload_fields(&app, &token, &[], &[("before", &bytes)])
            .await
            .status(),
        StatusCode::OK
    );
    let path = format!("/api/v1/rental-access/{token}/photos/before");
    for denied in [
        format!("/api/v1/rental-access/{other_token}/photos/before"),
        format!("/api/v1/rental-access/{token}/photos/identity"),
        format!("/api/v1/rental-access/{token}/photos/anything"),
        format!("/api/v1/rental-access/{}/photos/before", "a".repeat(64)),
        "/api/v1/rental-access/bad-token/photos/before".to_owned(),
    ] {
        assert_eq!(
            app.call("GET", &denied, None, false).await.status(),
            StatusCode::NOT_FOUND,
            "{denied}"
        );
    }
    for status in [
        "pending",
        "invoiced",
        "paid",
        "rejected",
        "closed",
        "cancelled",
    ] {
        sqlx::query("UPDATE rentals SET status=$1 WHERE id=$2")
            .bind(status)
            .bind(id)
            .execute(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::NOT_FOUND,
            "{status}"
        );
        assert_eq!(
            upload_fields(&app, &token, &[], &[("after", &bytes)])
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{status}"
        );
    }
    sqlx::query("UPDATE rentals SET status='approved',require_meter=FALSE WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        upload_fields(&app, &token, &[], &[("after", &bytes)])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE rentals SET require_meter=TRUE WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE rental_settings SET enabled=FALSE WHERE id=1")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        upload_fields(&app, &token, &[], &[("after", &bytes)])
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    sqlx::query("UPDATE rental_settings SET enabled=TRUE WHERE id=1")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::OK
    );
    sqlx::query("DELETE FROM rental_access_tokens WHERE rental_id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        upload_fields(&app, &token, &[], &[("after", &bytes)])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn slow_meter_upload_does_not_allow_unbounded_parallel_uploads() {
    use axum::body::Bytes;
    use std::task::Poll;
    let app = App::new().await;
    let id = asset(&app).await;
    let day = monday();
    let response = request(&app, id, day, day, "true", &photo()).await;
    let rental_id = body_json(response).await["id"].as_i64().unwrap();
    assert!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{rental_id}/approve"),
            None,
            true
        )
        .await
        .status()
        .is_success()
    );
    let token = approval_token(&app, rental_id).await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let mut started_tx = Some(started_tx);
    let slow = futures_util::stream::poll_fn(move |_| {
        if let Some(sender) = started_tx.take() {
            let _ = sender.send(());
        }
        Poll::<Option<Result<Bytes, std::io::Error>>>::Pending
    });
    let slow_request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/rental-access/{token}/photos"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from_stream(slow))
        .unwrap();
    let first = tokio::spawn(app.router.clone().oneshot(slow_request));
    tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    let second_request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/rental-access/{token}/photos"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(multipart(
            &[("kind", "before")],
            "photo",
            &photo(),
        )))
        .unwrap();
    let mut second = tokio::spawn(app.router.clone().oneshot(second_request));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut second)
            .await
            .is_err()
    );
    first.abort();
    let _ = first.await;
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.status(), StatusCode::OK);
}
