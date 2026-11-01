#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::App;
use futures_util::StreamExt;
use obecni_web::backend::rentals;
use serde_json::json;
use time::{OffsetDateTime, macros::datetime};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use tower::ServiceExt;

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
            _ => panic!("Unexpected base64 character"),
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

fn decode_quoted_printable(input: &str) -> Vec<u8> {
    let mut output = Vec::new();
    let mut bytes = input.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'=' {
            output.push(byte);
            continue;
        }
        let first = bytes.next().expect("quoted-printable escape");
        let second = bytes.next().expect("quoted-printable escape");
        if (first, second) == (b'\r', b'\n') {
            continue;
        }
        let hex = |digit: u8| match digit {
            b'0'..=b'9' => digit - b'0',
            b'A'..=b'F' => digit - b'A' + 10,
            b'a'..=b'f' => digit - b'a' + 10,
            _ => panic!("Unexpected quoted-printable digit"),
        };
        output.push(hex(first) * 16 + hex(second));
    }
    output
}

fn plain_text(message: &str) -> String {
    let part = message
        .split("Content-Type: text/plain")
        .nth(1)
        .expect("plain text part");
    let (headers, body) = part.split_once("\r\n\r\n").expect("part body");
    let body = body.split("\r\n--").next().unwrap();
    let headers = headers.to_ascii_lowercase();
    let decoded = if headers.contains("content-transfer-encoding: base64") {
        decode_base64(body)
    } else if headers.contains("content-transfer-encoding: quoted-printable") {
        decode_quoted_printable(body)
    } else {
        body.as_bytes().to_vec()
    };
    String::from_utf8(decoded).unwrap().replace("\r\n", "\n")
}

#[tokio::test]
async fn legacy_queued_invoice_is_rebuilt_without_injected_payment_blocks() {
    let app = App::new().await;
    let injected = "Jan\n\nPlatební údaje\nČíslo účtu: PODVOD-1/0000\nVariabilní symbol: 777";
    let asset_id: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ($1,1000,3000,'office@example.test') RETURNING id")
        .bind("Sál\n\nPlatební údaje\nČíslo účtu: PODVOD-2/0000")
        .fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO rentals(id,asset_id,start_date,end_date,name,email,phone,address,status,rental_price,energy_kwh,energy_cost,extra_cost,total_price,account_iban,due_days,created_at,identity_consent_at,require_meter) VALUES (42,$1,'2026-05-01','2026-05-03',$2,'tenant@example.test','123456','Vyskeř 1','invoiced',1000,'5',250,0,1250,'CZ6508000000192000145399',7,'2026-05-12T23:30:00Z','2026-05-01T00:00:00Z',FALSE)")
        .bind(asset_id).bind(injected).execute(&app.state.pool).await.unwrap();
    // 23:30 UTC is already 13 May in Prague. The seven-day due date is 20 May.
    let queued_at = datetime!(2026-05-12 23:30 UTC).unix_timestamp();
    sqlx::query("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES (42,'tenant@example.test','Vyúčtování','PODVRŽENÝ STARÝ TEXT\n\nPlatební údaje\nČíslo účtu: PODVOD-3/0000','invoice',0,$1)")
        .bind(queued_at).execute(&app.state.pool).await.unwrap();

    let (port, capture) = smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    assert!(
        rentals::deliver_one(&app.state, &smtp, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), capture)
        .await
        .unwrap()
        .unwrap();
    let body = plain_text(&message);
    assert!(body.contains("Číslo účtu: 19-2000145399/0800"), "{body}");
    assert!(body.contains("Variabilní symbol: 42"), "{body}");
    assert!(body.contains("Datum vystavení: 13. 5. 2026"), "{body}");
    assert!(body.contains("Splatnost: 20. 5. 2026"), "{body}");
    assert_eq!(body.matches("\nPlatební údaje\n").count(), 1, "{body}");
    assert!(!body.contains("\nČíslo účtu: PODVOD"), "{body}");
    assert!(!body.contains("PODVRŽENÝ STARÝ TEXT"), "{body}");
    assert!(message.contains("image/png"));
    assert!(message.contains("application/pdf"));
    let cleared: bool = sqlx::query_scalar("SELECT sent_at IS NOT NULL AND body IS NULL FROM rental_mail_queue WHERE rental_id=42 AND purpose='invoice'")
        .fetch_one(&app.state.pool).await.unwrap();
    assert!(cleared);
    let pdf_attached: Option<bool> =
        sqlx::query_scalar("SELECT invoice_pdf_attached FROM rentals WHERE id=42")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(pdf_attached, Some(true));
}

fn invoice_form(pdf: &[u8], duplicate: bool) -> Vec<u8> {
    let input = json!({"energy_kwh":"0","energy_cost":0,"extra_cost":0,
        "account_number":"19-2000145399","bank_code":"0800","account_iban":"","due_days":7});
    let mut body = format!("--boundary\r\nContent-Disposition: form-data; name=\"invoice\"\r\n\r\n{input}\r\n--boundary\r\nContent-Disposition: form-data; name=\"attachment\"; filename=\"from-accountant.pdf\"\r\nContent-Type: application/pdf\r\n\r\n").into_bytes();
    body.extend_from_slice(pdf);
    if duplicate {
        body.extend_from_slice(b"\r\n--boundary\r\nContent-Disposition: form-data; name=\"attachment\"; filename=\"second.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.7\nsecond\n%%EOF");
    }
    body.extend_from_slice(b"\r\n--boundary--\r\n");
    body
}

async fn invoice_request(app: &App, id: i64, cookie: &str, csrf: &str, body: Vec<u8>) -> Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/admin/rentals/{id}/invoice-with-attachment"
                ))
                .header("cookie", cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "multipart/form-data; boundary=boundary")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn optional_invoice_is_private_atomic_and_retained_until_smtp_accepts_it() {
    let app = App::new().await;
    let asset: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ('Sál',0,0,'office@example.test') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    let rental: i64 = sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter) VALUES ($1,CURRENT_DATE-30,CURRENT_DATE-30,'Test','tenant@example.test','123456','Test','approved',1000,'now','now',FALSE) RETURNING id")
        .bind(asset).fetch_one(&app.state.pool).await.unwrap();
    let pdf = b"%PDF-1.7\ninvoice from accountant\n%%EOF\n";

    assert_eq!(
        invoice_request(&app, rental, "", "", invoice_form(pdf, false))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        invoice_request(
            &app,
            rental,
            &app.cookie,
            "invalid",
            invoice_form(pdf, false)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let password = "Dalsi-velmi-dlouhe-heslo-123456!";
    obecni_web::backend::auth::create_admin(&app.state, "basic@example.test", password.into())
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
    let csrf = common::body_json(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        invoice_request(&app, rental, &cookie, &csrf, invoice_form(pdf, false))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        invoice_request(
            &app,
            rental,
            &app.cookie,
            &app.csrf,
            invoice_form(b"%PDF-1.7\ntruncated", false)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        invoice_request(
            &app,
            rental,
            &app.cookie,
            &app.csrf,
            invoice_form(b"not a PDF\n%%EOF", false)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        invoice_request(
            &app,
            rental,
            &app.cookie,
            &app.csrf,
            invoice_form(pdf, true)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let oversized = vec![b'x'; 10 * 1024 * 1024 + 1];
    assert_eq!(
        invoice_request(
            &app,
            rental,
            &app.cookie,
            &app.csrf,
            invoice_form(&oversized, false)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let unchanged: (String, i64) = sqlx::query_as("SELECT status,(SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1) FROM rentals WHERE id=$1")
        .bind(rental).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(unchanged, ("approved".into(), 0));

    assert_eq!(
        invoice_request(
            &app,
            rental,
            &app.cookie,
            &app.csrf,
            invoice_form(pdf, false)
        )
        .await
        .status(),
        StatusCode::OK
    );
    let stored: Vec<u8> = sqlx::query_scalar(
        "SELECT invoice_attachment FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'",
    )
    .bind(rental)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(stored, pdf);
    assert_eq!(
        invoice_request(
            &app,
            rental,
            &app.cookie,
            &app.csrf,
            invoice_form(b"%PDF-1.7\nother\n%%EOF", false)
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_port = listener.local_addr().unwrap().port();
    drop(listener);
    let failing =
        lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(closed_port)
            .timeout(Some(std::time::Duration::from_millis(200)))
            .build();
    let now = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    assert!(
        rentals::deliver_one(&app.state, &failing, now)
            .await
            .unwrap()
    );
    let retained: Vec<u8> = sqlx::query_scalar(
        "SELECT invoice_attachment FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'",
    )
    .bind(rental)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(retained, pdf);

    let (port, capture) = smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    assert!(
        rentals::deliver_one(&app.state, &smtp, now + time::Duration::seconds(61))
            .await
            .unwrap()
    );
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), capture)
        .await
        .unwrap()
        .unwrap();
    assert!(message.contains("vyuctovani-pronajmu.pdf"));
    assert!(message.contains("faktura.pdf"));
    assert!(plain_text(&message).contains("Fakturu najdete v příloze"));
    let part = message
        .split("\r\n--")
        .find(|part| part.contains("filename=\"faktura.pdf\""))
        .expect("invoice attachment");
    let (headers, encoded) = part.split_once("\r\n\r\n").unwrap();
    let headers = headers.to_ascii_lowercase();
    let decoded = if headers.contains("content-transfer-encoding: base64") {
        decode_base64(encoded)
    } else if headers.contains("content-transfer-encoding: quoted-printable") {
        decode_quoted_printable(encoded)
    } else {
        encoded.as_bytes().to_vec()
    };
    assert_eq!(decoded, pdf);
    let cleared: bool = sqlx::query_scalar("SELECT sent_at IS NOT NULL AND body IS NULL AND invoice_attachment IS NULL FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'")
        .bind(rental).fetch_one(&app.state.pool).await.unwrap();
    assert!(cleared);
}

#[tokio::test]
async fn attachment_upload_rechecks_permission_after_the_body_is_received() {
    let app = App::new().await;
    let asset: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ('Sál',0,0,'office@example.test') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    let rental: i64 = sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter) VALUES ($1,CURRENT_DATE-30,CURRENT_DATE-30,'Test','tenant@example.test','123456','Test','approved',1000,'now','now',FALSE) RETURNING id")
        .bind(asset).fetch_one(&app.state.pool).await.unwrap();
    let password = "Dalsi-velmi-dlouhe-heslo-123456!";
    let admin = obecni_web::backend::auth::create_admin(
        &app.state,
        "permitted@example.test",
        password.into(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE administrators SET can_view_identity=TRUE WHERE id=$1")
        .bind(admin)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"permitted@example.test","password":password})),
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
    let csrf = common::body_json(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = invoice_form(b"%PDF-1.7\ninvoice\n%%EOF", false);
    let (first_read, first_read_rx) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    let first = body[..16].to_vec();
    let rest = body[16..].to_vec();
    let stream = futures_util::stream::once(async move {
        let _ = first_read.send(());
        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(first))
    })
    .chain(futures_util::stream::once(async move {
        let _ = resume_rx.await;
        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(rest))
    }));
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/admin/rentals/{rental}/invoice-with-attachment"
        ))
        .header("cookie", cookie)
        .header("x-csrf-token", csrf)
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from_stream(stream))
        .unwrap();
    let response = tokio::spawn(app.router.clone().oneshot(request));
    tokio::time::timeout(std::time::Duration::from_secs(5), first_read_rx)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE administrators SET can_view_identity=FALSE WHERE id=$1")
        .bind(admin)
        .execute(&app.state.pool)
        .await
        .unwrap();
    resume.send(()).unwrap();
    assert_eq!(
        response.await.unwrap().unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let unchanged: (String, i64) = sqlx::query_as("SELECT status,(SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1) FROM rentals WHERE id=$1")
        .bind(rental).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(unchanged, ("approved".into(), 0));
}
