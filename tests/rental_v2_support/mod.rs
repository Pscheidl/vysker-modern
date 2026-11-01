#![allow(dead_code)]
use crate::common::{App, body_json};
use axum::{body::Body, http::Request, response::Response};
use serde_json::{Value, json};
use time::{Date, Duration, OffsetDateTime, Weekday};
use tower::ServiceExt;

pub fn monday() -> Date {
    let mut day = obecni_web::backend::today(OffsetDateTime::now_utc()) + Duration::days(10);
    while day.weekday() != Weekday::Monday {
        day += Duration::days(1)
    }
    day
}
pub async fn asset(app: &App, meter: bool) -> i64 {
    let response=app.call("POST","/api/v1/admin/rental-assets",Some(json!({"name":"Hodinový sál","description":"Test","rates":{"nonprofit":{"hourly_price":0,"weekend_day_price":0},"standard":{"hourly_price":80,"weekend_day_price":700}},"notification_email":"office@example.test","require_meter":meter,"active":true})),true).await;
    assert_eq!(response.status(), 200);
    body_json(response).await["id"].as_i64().unwrap()
}
pub fn admin_input(asset: i64, day: Date, start: Option<&str>, end: Option<&str>) -> Value {
    json!({"event_type":"standard","asset_id":asset,"start_date":day.to_string(),"end_date":day.to_string(),"start_time":start,"end_time":end,"name":"Skupina jógy","email":"yoga@example.test","phone":"+420 123 456 789","address":"Vyskeř 1","rental_price":25,"require_meter":false,"wait_for_payment":false,"recurrence_interval_days":0,"recurrence_until":null})
}
pub async fn create(app: &App, input: Value) -> Value {
    let response = app
        .call("POST", "/api/v1/admin/rentals", Some(input), true)
        .await;
    let status = response.status();
    let result = body_json(response).await;
    assert_eq!(status, 201, "{result}");
    result
}
pub async fn public(
    app: &App,
    asset: i64,
    day: Date,
    start: Option<&str>,
    end: Option<&str>,
) -> Response {
    let mut image = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(3, 3)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    let mut fields = vec![
        ("asset_id", asset.to_string()),
        ("event_type", "standard".into()),
        ("start_date", day.to_string()),
        ("end_date", day.to_string()),
        ("name", "Jan Test".into()),
        ("email", "jan@example.test".into()),
        ("phone", "123456789".into()),
        ("address", "Vyskeř 1".into()),
        ("consent", "true".into()),
    ];
    if let Some(v) = start {
        fields.push(("start_time", v.into()))
    }
    if let Some(v) = end {
        fields.push(("end_time", v.into()))
    }
    let mut body = Vec::new();
    for (key, value) in fields {
        body.extend_from_slice(
            format!(
                "--boundary\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        )
    }
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"identity\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n");
    body.extend_from_slice(image.get_ref());
    body.extend_from_slice(b"\r\n--boundary--\r\n");
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/rentals")
                .header("content-type", "multipart/form-data; boundary=boundary")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}
pub async fn set_payment(app: &App) {
    let response = app
        .call(
            "PUT",
            "/api/v1/admin/rental-payment-settings",
            Some(
                json!({"account_number":"19-2000145399","bank_code":"0800","iban":"","due_days":7}),
            ),
            true,
        )
        .await;
    assert!(response.status().is_success());
}
pub async fn smtp_capture() -> (u16, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        writer.write_all(b"220 localhost SMTP\r\n").await.unwrap();
        let mut body = String::new();
        let mut data = false;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            if data {
                if line == ".\r\n" {
                    writer.write_all(b"250 accepted\r\n").await.unwrap();
                    break;
                }
                body.push_str(&line)
            } else if line.starts_with("DATA") {
                data = true;
                writer.write_all(b"354 send message\r\n").await.unwrap()
            } else {
                writer.write_all(b"250 localhost\r\n").await.unwrap()
            }
        }
        body
    });
    (port, task)
}

/// Exercise the actual administrative multipart endpoint with an in-memory test image.
pub async fn admin_upload(app: &App, input: Value, include_identity: bool) -> Response {
    let mut body = format!(
        "--admin-boundary\r\nContent-Disposition: form-data; name=\"data\"\r\n\r\n{input}\r\n"
    )
    .into_bytes();
    if include_identity {
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(3, 3)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        body.extend_from_slice(b"--admin-boundary\r\nContent-Disposition: form-data; name=\"identity\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n");
        body.extend_from_slice(image.get_ref());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--admin-boundary--\r\n");
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/rentals")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=admin-boundary",
                )
                .header("cookie", &app.cookie)
                .header("x-csrf-token", &app.csrf)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}
