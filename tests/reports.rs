#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, body_json};
use http_body_util::BodyExt;
use image::{DynamicImage, ImageFormat};
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use obecni_web::backend::{self, reports};
use obecni_web::reports::{
    REPORT_MAP_LIMIT, REPORT_PHOTO_BYTES, REPORT_UPLOAD_BYTES, Report, ReportStatusFilter,
};
use serde_json::json;
use std::{io::Cursor, net::SocketAddr};
use time::OffsetDateTime;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use tower::ServiceExt;

async fn submit(
    app: &App,
    fields: &[(&str, &str)],
    photo: Option<&[u8]>,
    origin: Option<&str>,
    ip: &str,
) -> Response {
    let photos: Vec<&[u8]> = photo.into_iter().collect();
    submit_photos(app, fields, &photos, origin, ip).await
}

async fn submit_photos(
    app: &App,
    fields: &[(&str, &str)],
    photos: &[&[u8]],
    origin: Option<&str>,
    ip: &str,
) -> Response {
    let (token, cookie) = issue_token(app, None, ip).await;
    let mut fields_with_token = vec![("submission_token", token.as_str())];
    fields_with_token.extend_from_slice(fields);
    let bytes = multipart_body(&fields_with_token, photos);
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/reports")
        .header(
            "content-type",
            "multipart/form-data; boundary=report-boundary",
        )
        .header("cookie", cookie)
        .header("x-real-ip", "198.51.100.200")
        .header("x-forwarded-for", "198.51.100.201");
    if let Some(origin) = origin {
        request = request.header("origin", origin)
    }
    let mut request = request.body(Body::from(bytes)).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 12345)));
    app.router.clone().oneshot(request).await.unwrap()
}

async fn issue_token(app: &App, cookie: Option<&str>, ip: &str) -> (String, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/report-submission-token")
        .header("origin", ORIGIN);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let mut request = request.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 12345)));
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap().to_owned())
        .unwrap_or_else(|| cookie.unwrap_or_default().to_owned());
    let result = body_json(response).await;
    (result["token"].as_str().unwrap().to_owned(), cookie)
}

async fn submit_proof(
    app: &App,
    token: &str,
    cookie: Option<&str>,
    fields: &[(&str, &str)],
    ip: &str,
    authenticated: bool,
) -> Response {
    submit_proof_photos(app, token, cookie, fields, &[], ip, authenticated).await
}

async fn submit_proof_photos(
    app: &App,
    token: &str,
    cookie: Option<&str>,
    fields: &[(&str, &str)],
    photos: &[&[u8]],
    ip: &str,
    authenticated: bool,
) -> Response {
    let mut all_fields = vec![("submission_token", token)];
    all_fields.extend_from_slice(fields);
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/reports")
        .header("origin", ORIGIN)
        .header("x-report-token", token)
        .header(
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0",
        )
        .header("accept-language", "cs-CZ,cs;q=0.9")
        .header(
            "content-type",
            "multipart/form-data; boundary=report-boundary",
        );
    let mut cookies = cookie.unwrap_or_default().to_owned();
    if authenticated {
        cookies.push_str(&format!("; {}", app.cookie));
    }
    if !cookies.is_empty() {
        request = request.header("cookie", cookies);
    }
    let mut request = request
        .body(Body::from(multipart_body(&all_fields, photos)))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 12345)));
    app.router.clone().oneshot(request).await.unwrap()
}

fn multipart_body(fields: &[(&str, &str)], photos: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, value) in fields {
        bytes.extend_from_slice(format!("--report-boundary\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    for photo in photos {
        bytes.extend_from_slice(b"--report-boundary\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"photo.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n");
        bytes.extend_from_slice(photo);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"--report-boundary--\r\n");
    bytes
}

async fn resolve(
    app: &App,
    id: i64,
    fields: &[(&str, &str)],
    photos: &[&[u8]],
    authenticated: bool,
    csrf: bool,
) -> Response {
    let mut request = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/admin/reports/{id}/resolution"))
        .header(
            "content-type",
            "multipart/form-data; boundary=report-boundary",
        )
        .header("origin", ORIGIN);
    if authenticated {
        request = request.header("cookie", &app.cookie);
    }
    if csrf {
        request = request.header("x-csrf-token", &app.csrf);
    }
    app.router
        .clone()
        .oneshot(
            request
                .body(Body::from(multipart_body(fields, photos)))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn picture(format: ImageFormat, width: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(width, 2)
        .write_to(&mut bytes, format)
        .unwrap();
    bytes.into_inner()
}

fn photo_with_exif() -> Vec<u8> {
    let mut photo = picture(ImageFormat::Jpeg, 3);
    // Little-endian TIFF with orientation 6 and a private metadata marker.
    let mut exif =
        b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
    exif.extend_from_slice(b"private-gps-marker");
    let mut segment = vec![0xff, 0xe1];
    segment.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    segment.extend_from_slice(&exif);
    photo.splice(2..2, segment);
    photo
}

const VALID: &[(&str, &str)] = &[
    ("description", "Rozbitá lavička u cesty"),
    ("address", "Vyskeř 1"),
];
const ORIGIN: &str = "http://localhost:3000";

#[tokio::test]
async fn admin_resolution_is_atomic_public_and_keeps_original_report_and_photos() {
    let app = App::new().await;
    let original = body_json(
        submit(
            &app,
            VALID,
            Some(&picture(ImageFormat::Jpeg, 4)),
            Some(ORIGIN),
            "192.0.2.1",
        )
        .await,
    )
    .await;
    let id = original["id"].as_i64().unwrap();
    assert!(original["resolution_comment"].is_null());
    assert!(original["resolution_updated_at"].is_null());
    assert_eq!(original["resolution_photo_ids"], json!([]));
    assert_eq!(
        resolve(&app, id, &[("status", "resolved")], &[], false, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        resolve(&app, id, &[("status", "resolved")], &[], true, false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = resolve(
        &app,
        id,
        &[
            ("status", "resolved"),
            ("resolution_comment", " Vyměněná prkna a opravený nátěr. "),
        ],
        &[&photo_with_exif(), &picture(ImageFormat::Png, 5)],
        true,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let resolved = body_json(response).await;
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(
        resolved["resolution_comment"],
        "Vyměněná prkna a opravený nátěr."
    );
    assert!(resolved["resolution_updated_at"].is_string());
    assert_eq!(resolved["request_ip"], "192.0.2.1");
    assert_eq!(resolved["photo_count"], 1);
    assert_eq!(resolved["description"], original["description"]);
    assert_eq!(resolved["created_at"], original["created_at"]);
    let ids = resolved["resolution_photo_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 2);
    let first_id = ids[0].as_i64().unwrap();
    let retained_id = ids[1].as_i64().unwrap();
    let public = body_json(
        app.call("GET", &format!("/api/v1/reports/{id}"), None, false)
            .await,
    )
    .await;
    assert!(public.get("request_ip").is_none());
    assert_eq!(public["resolution_comment"], resolved["resolution_comment"]);
    assert_eq!(
        public["resolution_photo_ids"],
        resolved["resolution_photo_ids"]
    );
    let contract: Report = serde_json::from_value(public).unwrap();
    assert_eq!(contract.resolution_photo_urls().len(), 2);
    let image = app
        .call("GET", &contract.resolution_photo_urls()[0], None, false)
        .await;
    assert_eq!(image.headers()["content-type"], "image/jpeg");
    assert_eq!(image.headers()["cache-control"], "no-store");
    let bytes = image.into_body().collect().await.unwrap().to_bytes();
    assert!(
        !bytes
            .windows(18)
            .any(|window| window == b"private-gps-marker")
    );
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (2, 3));
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/reports/{}/resolution-photos/{first_id}", id + 1),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let unchanged = body_json(
        resolve(
            &app,
            id,
            &[
                ("status", "resolved"),
                ("resolution_comment", "Vyměněná prkna a opravený nátěr."),
            ],
            &[],
            true,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(unchanged, resolved);
    sqlx::query(
        "UPDATE issue_reports SET resolution_updated_at='2026-10-20T10:00:00Z' WHERE id=$1",
    )
    .bind(id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    for update in [json!({"hidden":false}), json!({"status":"resolved"})] {
        let unchanged = body_json(
            app.call(
                "PUT",
                &format!("/api/v1/admin/reports/{id}"),
                Some(update),
                true,
            )
            .await,
        )
        .await;
        assert_eq!(unchanged["resolution_updated_at"], "2026-10-20T10:00:00Z");
    }
    let changed = body_json(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({"status":"in_progress"})),
            true,
        )
        .await,
    )
    .await;
    assert_ne!(changed["resolution_updated_at"], "2026-10-20T10:00:00Z");
    assert_eq!(
        changed["resolution_comment"],
        resolved["resolution_comment"]
    );
    let removals = format!("[{first_id}]");
    let edited = body_json(
        resolve(
            &app,
            id,
            &[
                ("status", "resolved"),
                ("resolution_comment", "Hotovo."),
                ("remove_photo_ids", &removals),
            ],
            &[&picture(ImageFormat::Png, 6)],
            true,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(edited["resolution_photo_ids"].as_array().unwrap().len(), 2);
    assert_eq!(edited["resolution_photo_ids"][0], retained_id);
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/reports/{id}/resolution-photos/{first_id}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/reports/{id}/photo"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
    for (fields, photos) in [
        (
            vec![("status", "reported"), ("resolution_comment", "Neuložit")],
            vec![b"invalid photo".as_slice()],
        ),
        (vec![("status", "invalid")], vec![]),
        (
            vec![("resolution_comment", "A"), ("resolution_comment", "B")],
            vec![],
        ),
        (vec![("remove_photo_ids", "[999999]")], vec![]),
    ] {
        assert_eq!(
            resolve(&app, id, &fields, &photos, true, true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let oversized = vec![0; REPORT_PHOTO_BYTES + 1];
    assert_eq!(
        resolve(
            &app,
            id,
            &[("status", "reported")],
            &[&oversized],
            true,
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    // A database failure after status/comment changes and a deletion must roll
    // the entire resolution back, including its timestamp and old photographs.
    sqlx::query("ALTER TABLE issue_report_resolution_photos ADD CONSTRAINT test_reject_resolution CHECK (report_id <> 1) NOT VALID").execute(&app.state.pool).await.unwrap();
    let removals = format!("[{retained_id}]");
    assert_eq!(
        resolve(
            &app,
            id,
            &[
                ("status", "reported"),
                ("resolution_comment", "Neuložit"),
                ("remove_photo_ids", &removals)
            ],
            &[&picture(ImageFormat::Jpeg, 3)],
            true,
            true
        )
        .await
        .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let stored = body_json(app.call("GET", "/api/v1/admin/reports", None, true).await).await;
    assert_eq!(stored[0], edited);
    sqlx::query(
        "ALTER TABLE issue_report_resolution_photos DROP CONSTRAINT test_reject_resolution",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let hidden = body_json(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({"hidden":true})),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(
        hidden["resolution_updated_at"],
        edited["resolution_updated_at"]
    );
    for photo_id in edited["resolution_photo_ids"].as_array().unwrap() {
        let photo_id = photo_id.as_i64().unwrap();
        assert_eq!(
            app.call(
                "GET",
                &format!("/api/v1/reports/{id}/resolution-photos/{photo_id}"),
                None,
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        let path = format!("/api/v1/admin/reports/{id}/resolution-photos/{photo_id}");
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.call("GET", &path, None, true).await.status(),
            StatusCode::OK
        );
    }
    let ids = edited["resolution_photo_ids"].to_string();
    let cleared = body_json(
        resolve(
            &app,
            id,
            &[("resolution_comment", ""), ("remove_photo_ids", &ids)],
            &[],
            true,
            true,
        )
        .await,
    )
    .await;
    assert!(cleared["resolution_comment"].is_null());
    assert_eq!(cleared["resolution_photo_ids"], json!([]));
    assert_eq!(cleared["status"], "resolved");
    assert_eq!(cleared["photo_count"], 1);
}

#[tokio::test]
async fn migration_preserves_existing_photos_and_reports_without_photos() {
    let pool = common::test_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("report_photos_{}", &backend::auth::token()[..32]);
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('search_path', $1, true)")
        .bind(&schema)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE navigation_items(label TEXT,path TEXT,sort_order BIGINT,visible BOOLEAN)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0015_issue_reports.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let original = picture(ImageFormat::Jpeg, 3);
    sqlx::query("INSERT INTO issue_reports(description,address,request_ip,created_at,photo) VALUES ('První hlášení','Vyskeř','192.0.2.1','2026-10-25T12:00:00Z',$1),('Bez fotografie','Vyskeř','192.0.2.2','2026-10-25T12:01:00Z',NULL)")
        .bind(&original).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0016_issue_report_photos.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let photos: Vec<(i64, i64, Vec<u8>)> =
        sqlx::query_as("SELECT report_id,position,photo FROM issue_report_photos")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(photos, vec![(1, 1, original)]);
    let reports: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_reports")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(reports, 2);
    let old_column: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=$1 AND table_name='issue_reports' AND column_name='photo')")
        .bind(&schema).fetch_one(&mut *tx).await.unwrap();
    assert!(!old_column);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn accepts_exact_photo_and_total_limits_and_rejects_larger_uploads_atomically() {
    let app = App::new().await;
    app.call(
        "PUT",
        "/api/v1/admin/report-settings",
        Some(json!({"recipients":["photos@example.test"]})),
        true,
    )
    .await;
    let mut full = picture(ImageFormat::Jpeg, 2);
    // JPEG decoders ignore trailing bytes. This exercises upload limits with a
    // real decodable image without allocating large decoded pixel buffers.
    full.resize(REPORT_PHOTO_BYTES, 0);
    let mut remainder = picture(ImageFormat::Jpeg, 3);
    remainder.resize(REPORT_UPLOAD_BYTES - 3 * REPORT_PHOTO_BYTES, 0);
    let response = submit_photos(
        &app,
        VALID,
        &[&full, &full, &full, &remainder],
        Some(ORIGIN),
        "192.0.2.110",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let report = body_json(response).await;
    let id = report["id"].as_i64().unwrap();
    assert_eq!(report["photo_count"], 4);
    let body: String = sqlx::query_scalar("SELECT body FROM report_mail_queue WHERE report_id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    for position in 1..=4 {
        assert!(body.contains(&format!(
            "http://localhost:3000/api/v1/reports/{id}/photos/{position}"
        )));
    }
    full.push(0);
    let rejected = submit_photos(&app, VALID, &[&full], Some(ORIGIN), "192.0.2.111").await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let error = rejected.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&error).contains("15 MB"));
    full.pop();
    remainder.push(0);
    let rejected = submit_photos(
        &app,
        VALID,
        &[&full, &full, &full, &remainder],
        Some(ORIGIN),
        "192.0.2.112",
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let error = rejected.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&error).contains("50 MB"));
    let rejected = submit_photos(
        &app,
        VALID,
        &[&full, b"invalid photo"],
        Some(ORIGIN),
        "192.0.2.113",
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM issue_reports),(SELECT count(*) FROM issue_report_photos),(SELECT count(*) FROM report_mail_queue)")
        .fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(counts, (1, 4, 1));
}

#[tokio::test]
async fn searches_all_public_descriptions_and_addresses_before_pagination() {
    let app = App::new().await;
    sqlx::query("INSERT INTO issue_reports(description,address,reporter_name,request_ip,created_at) SELECT CASE WHEN i%2=0 THEN 'Rozbitá lampa' ELSE 'Poškozená lavička' END,CASE WHEN i=24 THEN 'U RYBNÍKA' ELSE 'Vyskeř' END,'Jméno nenajde','192.0.2.200','2026-10-25T12:00:00Z' FROM generate_series(1,24) i")
        .execute(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO issue_reports(description,address,request_ip,created_at,hidden) VALUES ('Značení 50%_!','Vyskeř','192.0.2.201','2026-10-25T12:00:00Z',FALSE),('Neveřejná lavička','Tajná adresa','192.0.2.202','2026-10-25T12:00:00Z',TRUE)")
        .execute(&app.state.pool).await.unwrap();
    let matches = body_json(
        app.call(
            "GET",
            "/api/v1/reports?q=LAVI&limit=5&offset=10",
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(matches.as_array().unwrap().len(), 2);
    assert_eq!(matches[0]["id"], 3);
    assert_eq!(matches[1]["id"], 1);
    assert_eq!(matches[0]["has_photo"], false);
    assert_eq!(matches[0]["photo_count"], 0);
    let matches = body_json(
        app.call("GET", "/api/v1/reports?q=%20rybn%20", None, false)
            .await,
    )
    .await;
    assert_eq!(matches.as_array().unwrap().len(), 1);
    assert_eq!(matches[0]["id"], 24);
    for query in ["%25", "_", "%21", "50%25_%21"] {
        let matches = body_json(
            app.call("GET", &format!("/api/v1/reports?q={query}"), None, false)
                .await,
        )
        .await;
        assert_eq!(matches.as_array().unwrap().len(), 1, "{query}");
        assert_eq!(matches[0]["description"], "Značení 50%_!");
    }
    for query in ["Tajn", "Neve", "192.0.2.200", "nenajde", "%27%20OR%201=1--"] {
        let matches = body_json(
            app.call("GET", &format!("/api/v1/reports?q={query}"), None, false)
                .await,
        )
        .await;
        assert_eq!(matches, json!([]), "{query}");
    }
    let all = body_json(
        app.call("GET", "/api/v1/reports?q=%20%20", None, false)
            .await,
    )
    .await;
    assert_eq!(all.as_array().unwrap().len(), 25);
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/reports?q={}", "a".repeat(201)),
            None,
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call("GET", "/api/v1/reports?q=x%0Ay", None, false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn map_covers_all_matching_pages_with_private_data_excluded_and_explicit_point_cap() {
    let app = App::new().await;
    sqlx::query("INSERT INTO issue_reports(description,address,latitude,longitude,reporter_name,request_ip,created_at) SELECT CASE WHEN i%2=0 THEN 'Rozbitá lavička' ELSE 'Lampa u cesty' END,CASE WHEN i=1 THEN 'U rybníka 50%_!' ELSE 'Vyskeř' END,50.529+i*0.000001,15.158,'Testovací autor','192.0.2.200','2026-10-25T12:00:00Z' FROM generate_series(1,25) i")
        .execute(&app.state.pool).await.unwrap();
    let full_description =
        "Lampa u rybníka. Druhá věta s podrobnostmi zůstává jen v detailu hlášení.";
    sqlx::query("UPDATE issue_reports SET description=$1 WHERE id=1")
        .bind(full_description)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issue_report_photos(report_id,position,photo) VALUES (1,1,$1)")
        .bind(picture(ImageFormat::Jpeg, 2))
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issue_reports(description,address,latitude,longitude,request_ip,created_at,hidden) VALUES ('Lampa bez zaměření','U rybníka',NULL,NULL,'192.0.2.201','2026-10-25T12:00:00Z',FALSE),('Rozbitá lavička','Vyskeř',NULL,NULL,'192.0.2.202','2026-10-25T12:00:00Z',FALSE),('Lampa skrytá','U rybníka',50.5,15.1,'192.0.2.203','2026-10-25T12:00:00Z',TRUE),('Lampa skrytá bez polohy','U rybníka',NULL,NULL,'192.0.2.204','2026-10-25T12:00:00Z',TRUE)")
        .execute(&app.state.pool).await.unwrap();
    let first_page = body_json(
        app.call("GET", "/api/v1/reports?limit=20", None, false)
            .await,
    )
    .await;
    assert!(
        !first_page
            .as_array()
            .unwrap()
            .iter()
            .any(|point| point["id"] == 1)
    );
    // List pagination does not constrain map coverage, even if passed explicitly.
    let all = body_json(
        app.call("GET", "/api/v1/reports/map?limit=1&offset=20", None, false)
            .await,
    )
    .await;
    assert_eq!(all["located_count"], 25);
    assert_eq!(all["unlocated_count"], 2);
    assert_eq!(all["truncated"], false);
    let points = all["points"].as_array().unwrap();
    assert_eq!(points.len(), 25);
    assert_eq!(points[0]["id"], 25);
    let oldest = points.last().unwrap();
    assert_eq!(oldest["id"], 1);
    assert_eq!(oldest["latitude"], 50.529001);
    assert_eq!(oldest["longitude"], 15.158);
    assert_eq!(oldest["description"], "Lampa u rybníka...");
    assert_eq!(oldest["has_photo"], true);
    assert_eq!(oldest["status"], "reported");
    assert!(
        points
            .iter()
            .all(|point| point["description"].as_str().unwrap().chars().count() <= 120)
    );
    for point in points {
        assert!(point.get("request_ip").is_none());
        assert!(point.get("reporter_name").is_none());
        assert!(point.get("photo").is_none());
    }
    assert!(!all.to_string().contains("192.0.2."));
    let detail = body_json(app.call("GET", "/api/v1/reports/1", None, false).await).await;
    assert_eq!(detail["description"], full_description);
    let matches = body_json(
        app.call("GET", "/api/v1/reports/map?q=LAMPA", None, false)
            .await,
    )
    .await;
    assert_eq!(matches["located_count"], 13);
    assert_eq!(matches["unlocated_count"], 1);
    assert_eq!(matches["points"].as_array().unwrap().len(), 13);
    let matches = body_json(
        app.call("GET", "/api/v1/reports/map?q=rybn", None, false)
            .await,
    )
    .await;
    assert_eq!(matches["located_count"], 1);
    assert_eq!(matches["unlocated_count"], 1);
    assert_eq!(matches["points"][0]["id"], 1);
    for query in ["%25", "_", "%21"] {
        let matches = body_json(
            app.call(
                "GET",
                &format!("/api/v1/reports/map?q={query}"),
                None,
                false,
            )
            .await,
        )
        .await;
        assert_eq!(matches["located_count"], 1);
        assert_eq!(matches["unlocated_count"], 0);
        assert_eq!(matches["points"][0]["id"], 1);
    }
    for query in ["skryt", "192.0.2.200", "Testovac", "nenalezeno"] {
        let matches = body_json(
            app.call(
                "GET",
                &format!("/api/v1/reports/map?q={query}"),
                None,
                false,
            )
            .await,
        )
        .await;
        assert_eq!(
            matches,
            json!({"points":[],"located_count":0,"unlocated_count":0,"truncated":false})
        );
    }
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/reports/map?q={}", "a".repeat(201)),
            None,
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    sqlx::query("INSERT INTO issue_reports(description,address,latitude,longitude,request_ip,created_at) SELECT 'Limit mapy','Vyskeř',50.529,15.158,'192.0.2.210','2026-10-25T12:00:00Z' FROM generate_series(1,$1::bigint)")
        .bind(REPORT_MAP_LIMIT + 1).execute(&app.state.pool).await.unwrap();
    let limited = body_json(
        app.call("GET", "/api/v1/reports/map?q=limit", None, false)
            .await,
    )
    .await;
    assert_eq!(limited["located_count"], REPORT_MAP_LIMIT + 1);
    assert_eq!(limited["unlocated_count"], 0);
    assert_eq!(limited["truncated"], true);
    let points = limited["points"].as_array().unwrap();
    assert_eq!(points.len(), REPORT_MAP_LIMIT as usize);
    assert_eq!(points.first().unwrap()["id"], 29 + REPORT_MAP_LIMIT + 1);
    assert_eq!(points.last().unwrap()["id"], 31);
}

#[tokio::test]
async fn coordinate_addresses_roundtrip_and_reject_invalid_or_conflicting_locations() {
    let app = App::new().await;
    let valid = [
        ("50.529509, 15.158555", 50.529509, 15.158555),
        (" +50.529509   +15.158555 ", 50.529509, 15.158555),
        ("50.529509;15.158555", 50.529509, 15.158555),
        ("50,529509; 15,158555", 50.529509, 15.158555),
        ("-90, +180", -90.0, 180.0),
        (".5, 15.", 0.5, 15.0),
    ];
    for (index, (address, latitude, longitude)) in valid.into_iter().enumerate() {
        let response = submit(
            &app,
            &[("description", "Poškozená cesta"), ("address", address)],
            None,
            Some(ORIGIN),
            &format!("192.0.2.{}", index + 50),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED, "{address}");
        let report = body_json(response).await;
        assert_eq!(report["latitude"], latitude);
        assert_eq!(report["longitude"], longitude);
        assert_eq!(report["address"], format!("{latitude:.6}, {longitude:.6}"));
        let id = report["id"].as_i64().unwrap();
        assert_eq!(
            body_json(
                app.call("GET", &format!("/api/v1/reports/{id}"), None, false)
                    .await
            )
            .await,
            report
        );
    }
    for (index, address) in ["91, 15", "50, -181", "-90.1; 15", "90,000001; 15,0"]
        .into_iter()
        .enumerate()
    {
        let response = submit(
            &app,
            &[("description", "Cesta"), ("address", address)],
            None,
            Some(ORIGIN),
            &format!("192.0.2.{}", index + 70),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{address}");
    }
    for (index, (latitude, longitude)) in [("50.529511", "15.158555"), ("50.529509", "15.158557")]
        .into_iter()
        .enumerate()
    {
        let response = submit(
            &app,
            &[
                ("description", "Cesta"),
                ("address", "50.529509, 15.158555"),
                ("latitude", latitude),
                ("longitude", longitude),
            ],
            None,
            Some(ORIGIN),
            &format!("192.0.2.{}", index + 80),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = submit(
        &app,
        &[
            ("description", "Cesta"),
            ("address", "50.529509, 15.158555"),
            ("latitude", "50.529510"),
            ("longitude", "15.158554"),
        ],
        None,
        Some(ORIGIN),
        "192.0.2.90",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let report = body_json(response).await;
    assert_eq!(report["latitude"], 50.529509);
    assert_eq!(report["longitude"], 15.158555);
    for (index, address) in ["5. května 1", "Vyskeř 14, u rybníka", "123"]
        .into_iter()
        .enumerate()
    {
        let response = submit(
            &app,
            &[("description", "Cesta"), ("address", address)],
            None,
            Some(ORIGIN),
            &format!("192.0.2.{}", index + 100),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let report = body_json(response).await;
        assert_eq!(report["address"], address);
        assert!(report["latitude"].is_null());
        assert!(report["longitude"].is_null());
    }
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_reports")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, 10);
}

#[tokio::test]
async fn anonymous_reports_are_public_but_ip_and_hidden_reports_are_private() {
    let app = App::new().await;
    let settings = body_json(
        app.call("GET", "/api/v1/admin/report-settings", None, true)
            .await,
    )
    .await;
    assert_eq!(settings, json!({"recipients":[]}));
    let response = submit_photos(
        &app,
        VALID,
        &[&photo_with_exif(), &picture(ImageFormat::Png, 4)],
        Some(ORIGIN),
        "192.0.2.10",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let report = body_json(response).await;
    let id = report["id"].as_i64().unwrap();
    assert!(report.get("reporter_name").is_none());
    assert!(report.get("request_ip").is_none());
    assert!(report.get("hidden").is_none());
    assert_eq!(report["status"], "reported");
    assert_eq!(report["has_photo"], true);
    assert_eq!(report["photo_count"], 2);
    let contract: Report = serde_json::from_value(report.clone()).unwrap();
    assert_eq!(
        contract.photo_urls(),
        [
            format!("/api/v1/reports/{id}/photos/1"),
            format!("/api/v1/reports/{id}/photos/2"),
        ]
    );
    OffsetDateTime::parse(
        report["created_at"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let public = body_json(app.call("GET", "/api/v1/reports", None, false).await).await;
    assert_eq!(public, json!([report]));
    assert_eq!(
        body_json(
            app.call("GET", &format!("/api/v1/reports/{id}"), None, false)
                .await
        )
        .await,
        report
    );
    let photo = app
        .call("GET", &format!("/api/v1/reports/{id}/photo"), None, false)
        .await;
    assert_eq!(photo.headers()["content-type"], "image/jpeg");
    assert_eq!(photo.headers()["cache-control"], "no-store");
    let bytes = photo.into_body().collect().await.unwrap().to_bytes();
    assert!(
        !bytes
            .windows(18)
            .any(|window| window == b"private-gps-marker")
    );
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (2, 3));
    let first = app
        .call(
            "GET",
            &format!("/api/v1/reports/{id}/photos/1"),
            None,
            false,
        )
        .await;
    assert_eq!(first.into_body().collect().await.unwrap().to_bytes(), bytes);
    let second = app
        .call(
            "GET",
            &format!("/api/v1/reports/{id}/photos/2"),
            None,
            false,
        )
        .await;
    assert_eq!(second.headers()["content-type"], "image/jpeg");
    let second = second.into_body().collect().await.unwrap().to_bytes();
    let decoded = image::load_from_memory(&second).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (4, 2));
    for position in [-1, 0, 3] {
        assert_eq!(
            app.call(
                "GET",
                &format!("/api/v1/reports/{id}/photos/{position}"),
                None,
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM report_mail_queue")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(pending, 0);
    let private = body_json(app.call("GET", "/api/v1/admin/reports", None, true).await).await;
    assert_eq!(private[0]["request_ip"], "192.0.2.10");
    assert_eq!(private[0]["hidden"], false);
    assert_eq!(private[0]["photo_count"], 2);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({"status":"in_progress","hidden":false})),
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        body_json(
            app.call("GET", &format!("/api/v1/reports/{id}"), None, false)
                .await
        )
        .await["status"],
        "in_progress"
    );
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({"status":"resolved","hidden":true})),
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    let changed = body_json(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({"status":"in_progress"})),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(changed["hidden"], true);
    let changed = body_json(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({"hidden":true})),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(changed["status"], "in_progress");
    assert_eq!(changed["photo_count"], 2);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    for path in [
        format!("/api/v1/reports/{id}"),
        format!("/api/v1/reports/{id}/photo"),
        format!("/api/v1/reports/{id}/photos/1"),
        format!("/api/v1/reports/{id}/photos/2"),
    ] {
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        body_json(app.call("GET", "/api/v1/reports", None, false).await).await,
        json!([])
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/reports/{id}/photo"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/reports/{id}/photo"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    for position in [1, 2] {
        let path = format!("/api/v1/admin/reports/{id}/photos/{position}");
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.call("GET", &path, None, true).await.status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn validates_locations_text_origins_files_and_admin_permissions() {
    let app = App::new().await;
    let invalid: Vec<Vec<(&str, &str)>> = vec![
        vec![("address", "Vyskeř 1")],
        vec![("description", " "), ("address", "Vyskeř 1")],
        vec![("description", "Závada")],
        vec![("description", "Závada"), ("latitude", "50")],
        vec![
            ("description", "Závada"),
            ("latitude", "NaN"),
            ("longitude", "15"),
        ],
        vec![
            ("description", "Závada"),
            ("latitude", "91"),
            ("longitude", "15"),
        ],
        vec![
            ("description", "Závada"),
            ("latitude", "50"),
            ("longitude", "181"),
        ],
        vec![
            ("description", "Závada"),
            ("address", "Vyskeř"),
            ("description", "Jiný popis"),
        ],
        vec![
            ("description", "Závada"),
            ("address", "Vyskeř"),
            ("hidden", "true"),
        ],
        vec![
            ("description", "Závada"),
            ("address", "Vyskeř"),
            ("reporter_name", "Jméno\nNavíc"),
        ],
    ];
    for (index, fields) in invalid.iter().enumerate() {
        assert_eq!(
            submit(
                &app,
                fields,
                None,
                Some(ORIGIN),
                &format!("192.0.2.{}", index + 1)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "{fields:?}"
        );
    }
    for origin in [None, Some("https://evil.test"), Some("null")] {
        assert_eq!(
            submit(&app, VALID, None, origin, "192.0.2.100")
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for (index, bytes) in [
        b"<svg onload='alert(1)'/>".to_vec(),
        picture(ImageFormat::Gif, 2),
        picture(ImageFormat::Png, 8193),
        vec![0; REPORT_PHOTO_BYTES + 1],
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            submit(
                &app,
                VALID,
                Some(bytes),
                Some(ORIGIN),
                &format!("192.0.2.{}", index + 20)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for (index, format) in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP]
        .into_iter()
        .enumerate()
    {
        let response = submit(
            &app,
            &[
                ("description", "Výtluk"),
                ("latitude", "50.532"),
                ("longitude", "15.161"),
                ("reporter_name", " Jana "),
            ],
            Some(&picture(format, 2)),
            Some(ORIGIN),
            &format!("192.0.2.{}", index + 30),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let report = body_json(response).await;
        assert_eq!(report["latitude"], 50.532);
        assert!(report.get("reporter_name").is_none());
        let name: Option<String> =
            sqlx::query_scalar("SELECT reporter_name FROM issue_reports WHERE id=$1")
                .bind(report["id"].as_i64().unwrap())
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
        assert_eq!(name.as_deref(), Some("Jana"));
        assert!(report["address"].is_null());
    }
    assert_eq!(
        app.call("GET", "/api/v1/admin/reports", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call("GET", "/api/v1/admin/report-settings", None, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "PUT",
            "/api/v1/admin/report-settings",
            Some(json!({"recipients":["a@example.test"]})),
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "PUT",
            "/api/v1/admin/report-settings",
            Some(json!({"recipients":["not-an-email"]})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "PUT",
            "/api/v1/admin/reports/1",
            Some(json!({"status":"bad","hidden":false})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call("GET", "/api/v1/reports?limit=101", None, false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn submission_uses_trusted_request_peer_and_recipient_snapshots_are_atomic() {
    let mut app = App::new().await;
    let settings = app.call("PUT", "/api/v1/admin/report-settings", Some(json!({"recipients":[" Second@Example.test ","first@example.test","FIRST@example.test"]})), true).await;
    assert_eq!(settings.status(), StatusCode::OK);
    assert_eq!(
        body_json(settings).await,
        json!({"recipients":["first@example.test","second@example.test"]})
    );
    for _ in 0..5 {
        assert_eq!(
            submit(&app, VALID, None, Some(ORIGIN), "192.0.2.1")
                .await
                .status(),
            StatusCode::CREATED
        );
    }
    assert_eq!(
        submit(&app, VALID, None, Some(ORIGIN), "192.0.2.2")
            .await
            .status(),
        StatusCode::CREATED
    );
    app.call(
        "PUT",
        "/api/v1/admin/report-settings",
        Some(json!({"recipients":[]})),
        true,
    )
    .await;
    assert_eq!(
        submit(&app, VALID, None, Some(ORIGIN), "192.0.2.2")
            .await
            .status(),
        StatusCode::CREATED
    );
    let pending: Vec<(String, String)> =
        sqlx::query_as("SELECT recipient,body FROM report_mail_queue ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(pending.len(), 12);
    assert!(!pending[0].1.contains("Kdo nahlásil"));
    assert!(pending[0].1.contains("Rozbitá lavička"));
    assert!(pending[0].1.contains("Vyskeř 1"));
    assert!(
        pending[0]
            .1
            .contains("http://localhost:3000/hlaseni-zavad/")
    );
    assert!(!pending[0].1.contains("192.0.2."));
    let mut config = (*app.state.config).clone();
    config.trusted_proxy = Some("127.0.0.1".parse().unwrap());
    app.state.config = std::sync::Arc::new(config);
    app.router = backend::router(app.state.clone());
    let report = body_json(submit(&app, VALID, None, Some(ORIGIN), "127.0.0.1").await).await;
    let stored: String = sqlx::query_scalar("SELECT request_ip FROM issue_reports WHERE id=$1")
        .bind(report["id"].as_i64().unwrap())
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, "198.51.100.200");
    // Failure of the outbox write rolls the report back in the same transaction.
    app.call(
        "PUT",
        "/api/v1/admin/report-settings",
        Some(json!({"recipients":["a@example.test"]})),
        true,
    )
    .await;
    sqlx::query("ALTER TABLE report_mail_queue ADD CONSTRAINT test_reject CHECK (recipient <> 'a@example.test')").execute(&app.state.pool).await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_reports")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        submit_photos(
            &app,
            VALID,
            &[
                &picture(ImageFormat::Jpeg, 2),
                &picture(ImageFormat::Png, 3)
            ],
            Some(ORIGIN),
            "192.0.2.3"
        )
        .await
        .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_reports")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let photos: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_report_photos")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(photos, 0);
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

#[tokio::test]
async fn notification_retries_and_reclaims_expired_leases_without_newsletter_configuration() {
    let app = App::new().await;
    app.call(
        "PUT",
        "/api/v1/admin/report-settings",
        Some(json!({"recipients":["one@example.test","two@example.test"]})),
        true,
    )
    .await;
    assert_eq!(
        submit(&app, VALID, None, Some(ORIGIN), "192.0.2.1")
            .await
            .status(),
        StatusCode::CREATED
    );
    let queued: (String, String) =
        sqlx::query_as("SELECT subject,body FROM report_mail_queue ORDER BY id LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(queued.0, "Vyskeř: nový podnět nebo závada č. 1");
    assert!(queued.1.starts_with("Nový podnět nebo závada č. 1\n"));
    let now = OffsetDateTime::now_utc();
    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = unused.local_addr().unwrap().port();
    drop(unused);
    let failing = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .timeout(Some(std::time::Duration::from_millis(200)))
        .build();
    assert!(
        reports::deliver_one(&app.state, &failing, now)
            .await
            .unwrap()
    );
    let failed: (i64,i64,i64,Option<String>,Option<i64>) = sqlx::query_as("SELECT attempts,next_attempt_at,locked_until,body,sent_at FROM report_mail_queue ORDER BY id LIMIT 1").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(failed.0, 1);
    assert_eq!(failed.1, now.unix_timestamp() + 60);
    assert_eq!(failed.2, 0);
    assert!(failed.3.is_some());
    assert!(failed.4.is_none());
    // Simulate a worker lost while holding the other message.
    sqlx::query("UPDATE report_mail_queue SET locked_until=$1,lock_token='expired-worker' WHERE recipient='two@example.test'").bind(now.unix_timestamp()-1).execute(&app.state.pool).await.unwrap();
    let mut state = app.state.clone();
    for (index, recipient) in ["one@example.test", "two@example.test"]
        .into_iter()
        .enumerate()
    {
        let (port, capture) = smtp_capture().await;
        let mut config = (*state.config).clone();
        config.privacy = None;
        config.smtp_port = port;
        state.config = std::sync::Arc::new(config);
        let mut worker = backend::mail::Worker::new(&state).unwrap();
        assert!(
            worker
                .deliver_one(&state, now + time::Duration::seconds(61))
                .await
                .unwrap()
        );
        let message = tokio::time::timeout(std::time::Duration::from_secs(3), capture)
            .await
            .unwrap()
            .unwrap();
        assert!(message.contains(&format!("To: {recipient}")));
        assert!(!message.contains(if index == 0 {
            "two@example.test"
        } else {
            "one@example.test"
        }));
        assert!(message.contains("Message-ID: <vysker-report-"));
    }
    let sent: i64 = sqlx::query_scalar("SELECT count(*) FROM report_mail_queue WHERE sent_at IS NOT NULL AND body IS NULL AND lock_token IS NULL AND locked_until=0").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(sent, 2);
}

#[tokio::test]
async fn report_proofs_are_bound_expiring_atomic_and_idempotent_across_restart() {
    let mut app = App::new().await;
    let (token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    let (_, other_cookie) = issue_token(&app, None, "192.0.2.1").await;
    let response = submit_proof(&app, &token, Some(&other_cookie), VALID, "192.0.2.1", false).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(response).await["code"], "report_token_invalid");
    let changed_last = if cookie.ends_with('0') { '1' } else { '0' };
    let forged = format!("{}{changed_last}", &cookie[..cookie.len() - 1]);
    let response = submit_proof(&app, &token, Some(&forged), VALID, "192.0.2.1", false).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // A changed network does not invalidate a browser's pending form.
    let (first, second) = tokio::join!(
        submit_proof(&app, &token, Some(&cookie), VALID, "192.0.2.2", false),
        submit_proof(&app, &token, Some(&cookie), VALID, "192.0.2.2", false),
    );
    let statuses = [first.status(), second.status()];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::OK));
    let first = body_json(first).await;
    assert_eq!(first, body_json(second).await);
    app.state = backend::Backend::new(app.state.pool.clone(), (*app.state.config).clone());
    app.router = backend::router(app.state.clone());
    let retry = submit_proof(&app, &token, Some(&cookie), VALID, "192.0.2.3", false).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(body_json(retry).await, first);
    let recovered =
        submit_proof(&app, &token, Some(&other_cookie), VALID, "192.0.2.4", false).await;
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(body_json(recovered).await, first);
    let changed = submit_proof(
        &app,
        &token,
        Some(&cookie),
        &[("description", "Jiné hlášení"), ("address", "Vyskeř 2")],
        "192.0.2.1",
        false,
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert!(body_json(changed).await.get("code").is_none());
    let (fresh, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    assert_eq!(
        submit_proof(&app, &fresh, Some(&cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::OK
    );
    let (expired, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    sqlx::query("UPDATE report_submission_tokens SET expires_at=0 WHERE token_hash=$1")
        .bind(backend::auth::hash(&expired))
        .execute(&app.state.pool)
        .await
        .unwrap();
    let response = submit_proof(&app, &expired, Some(&cookie), VALID, "192.0.2.1", false).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(response).await["code"], "report_token_invalid");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_reports")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn sender_context_is_private_and_browser_fields_do_not_claim_identity() {
    let app = App::new().await;
    app.call(
        "PUT",
        "/api/v1/admin/report-settings",
        Some(json!({"recipients":["test@example.test"]})),
        true,
    )
    .await;
    let (token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    let response = submit_proof(
        &app,
        &token,
        Some(&cookie),
        &[
            ("description", "Rozbitá lampa"),
            ("address", "Vyskeř 50"),
            ("reporter_name", "Soukromé jméno"),
            ("reporter_email", "Kontakt@Example.test"),
            ("reporter_phone", "+420 123  456 789"),
            ("client_timezone", "Europe/Prague"),
        ],
        "192.0.2.1",
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let public = body_json(response).await;
    let id = public["id"].as_i64().unwrap();
    let private_fields = [
        "reporter_name",
        "reporter_email",
        "reporter_phone",
        "request_ip",
        "request_user_agent",
        "request_accept_language",
        "client_timezone",
        "browser_id_hash",
        "browser_fingerprint_hash",
        "authenticated_admin_id",
        "authenticated_admin_email",
        "metadata_recorded_at",
    ];
    for field in private_fields {
        assert!(public.get(field).is_none(), "{field}");
    }
    for url in [
        format!("/api/v1/reports/{id}"),
        "/api/v1/reports".to_owned(),
        "/api/v1/reports/map".to_owned(),
    ] {
        let response = body_json(app.call("GET", &url, None, false).await)
            .await
            .to_string();
        for value in [
            "Soukromé jméno",
            "kontakt@example.test",
            "+420 123 456 789",
            "192.0.2.1",
            "Chrome/141",
            "Europe/Prague",
            "admin@vysker.test",
        ] {
            assert!(!response.contains(value), "{url} leaked {value}");
        }
    }
    let report =
        body_json(app.call("GET", "/api/v1/admin/reports", None, true).await).await[0].clone();
    assert_eq!(report["reporter_name"], "Soukromé jméno");
    assert_eq!(report["reporter_email"], "kontakt@example.test");
    assert_eq!(report["reporter_phone"], "+420 123 456 789");
    assert_eq!(report["authenticated_admin_id"], app.admin_id);
    assert_eq!(report["authenticated_admin_email"], "admin@vysker.test");
    assert_eq!(report["client_timezone"], "Europe/Prague");
    assert_eq!(report["request_ip"], "192.0.2.1");
    assert_eq!(report["browser_id_hash"].as_str().unwrap().len(), 64);
    assert_eq!(
        report["browser_fingerprint_hash"].as_str().unwrap().len(),
        64
    );
    let mail: String = sqlx::query_scalar("SELECT body FROM report_mail_queue WHERE report_id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    for private in [
        "Soukromé jméno",
        "kontakt@example.test",
        "+420 123 456 789",
        "192.0.2.1",
        "Chrome/141",
        "Europe/Prague",
        "admin@vysker.test",
    ] {
        assert!(!mail.contains(private));
    }
    let (token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    let invalid_session = format!("{cookie}; obec_session={}", "f".repeat(64));
    let response = submit_proof(
        &app,
        &token,
        Some(&invalid_session),
        VALID,
        "192.0.2.1",
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let report =
        body_json(app.call("GET", "/api/v1/admin/reports", None, true).await).await[0].clone();
    assert!(report["authenticated_admin_id"].is_null());
    assert!(report["reporter_name"].is_null());
}

#[tokio::test]
async fn report_quotas_target_browser_and_allow_neighbours_on_shared_ip() {
    let mut app = App::new().await;
    let (mut token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    for index in 0..10 {
        if index > 0 {
            token = issue_token(&app, Some(&cookie), "192.0.2.1").await.0;
        }
        let description = format!("Různá závada {index}");
        let response = submit_proof(
            &app,
            &token,
            Some(&cookie),
            &[("description", &description), ("address", "Vyskeř")],
            "192.0.2.1",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }
    let (token, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    app.state = backend::Backend::new(app.state.pool.clone(), (*app.state.config).clone());
    app.router = backend::router(app.state.clone());
    assert_eq!(
        submit_proof(&app, &token, Some(&cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let (other_token, other_cookie) = issue_token(&app, None, "192.0.2.1").await;
    // Identical headers and coarse fingerprint never block a different browser.
    assert_eq!(
        submit_proof(
            &app,
            &other_token,
            Some(&other_cookie),
            VALID,
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let (token, other_cookie) = issue_token(&app, None, "192.0.2.1").await;
    let response = submit_proof(
        &app,
        &token,
        Some(&other_cookie),
        &[
            ("description", "Závada"),
            ("address", "Vyskeř"),
            ("website", "https://spam.example"),
        ],
        "192.0.2.1",
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Validation does not consume the one-time proof.
    assert_eq!(
        submit_proof(&app, &token, Some(&other_cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn cookieless_submission_uses_same_origin_header_and_short_lived_network_bound_proof() {
    let app = App::new().await;
    let (token, _) = issue_token(&app, None, "192.0.2.1").await;
    let mut no_header = Request::builder()
        .method("POST")
        .uri("/api/v1/reports")
        .header("origin", ORIGIN)
        .header(
            "content-type",
            "multipart/form-data; boundary=report-boundary",
        )
        .body(Body::from(multipart_body(
            &[
                ("submission_token", &token),
                ("description", "Lavička"),
                ("address", "Vyskeř"),
            ],
            &[],
        )))
        .unwrap();
    no_header.extensions_mut().insert(ConnectInfo(
        "192.0.2.1:12345".parse::<SocketAddr>().unwrap(),
    ));
    assert_eq!(
        app.router
            .clone()
            .oneshot(no_header)
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        submit_proof(&app, &token, None, VALID, "192.0.2.2", false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = submit_proof(&app, &token, None, VALID, "192.0.2.1", false).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = body_json(response).await;
    let retry = submit_proof(&app, &token, None, VALID, "192.0.2.2", false).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(body_json(retry).await, created);
    let report =
        body_json(app.call("GET", "/api/v1/admin/reports", None, true).await).await[0].clone();
    assert!(report["browser_id_hash"].is_null());
    assert!(report["browser_fingerprint_hash"].is_string());
}

#[tokio::test]
async fn report_abuse_cleanup_expires_metadata_without_changing_report_or_name() {
    let app = App::new().await;
    let (token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    let response = submit_proof(
        &app,
        &token,
        Some(&cookie),
        &[
            ("description", "Závada"),
            ("address", "Vyskeř"),
            ("reporter_name", "Nepovinné jméno"),
            ("reporter_email", "kontakt@example.test"),
            ("reporter_phone", "123 456 789"),
        ],
        "192.0.2.1",
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let report = body_json(response).await;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let expires: i64 =
        sqlx::query_scalar("SELECT metadata_expires_at FROM issue_reports WHERE id=$1")
            .bind(report["id"].as_i64().unwrap())
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(expires >= now + 29 * 86400 && expires <= now + 30 * 86400);
    backend::report_abuse::maintenance(&app.state.pool, now + 31 * 86400)
        .await
        .unwrap();
    let private =
        body_json(app.call("GET", "/api/v1/admin/reports", None, true).await).await[0].clone();
    assert_eq!(private["request_ip"], "");
    assert_eq!(private["reporter_name"], "Nepovinné jméno");
    assert_eq!(private["reporter_email"], "kontakt@example.test");
    assert_eq!(private["reporter_phone"], "123 456 789");
    for field in [
        "request_user_agent",
        "request_accept_language",
        "client_timezone",
        "browser_id_hash",
        "browser_fingerprint_hash",
        "authenticated_admin_id",
        "authenticated_admin_email",
    ] {
        assert!(private[field].is_null(), "{field}");
    }
    assert_eq!(
        body_json(
            app.call(
                "GET",
                &format!("/api/v1/reports/{}", report["id"]),
                None,
                false
            )
            .await
        )
        .await,
        report
    );
    for query in [
        "SELECT count(*) FROM report_submission_tokens",
        "SELECT count(*) FROM report_submission_duplicates",
        "SELECT count(*) FROM report_abuse_rates",
        "SELECT count(*) FROM report_abuse_events",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{query}");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM report_abuse_settings")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "site key must survive maintenance");
}

#[tokio::test]
async fn token_issuance_is_same_origin_rate_limited_and_expired_rows_are_collected() {
    let app = App::new().await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/report-submission-token")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.router.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let (token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    sqlx::query("UPDATE report_submission_tokens SET retain_until=0 WHERE token_hash=$1")
        .bind(backend::auth::hash(&token))
        .execute(&app.state.pool)
        .await
        .unwrap();
    issue_token(&app, Some(&cookie), "192.0.2.1").await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM report_submission_tokens")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    sqlx::query("UPDATE report_abuse_rates SET hits=120 WHERE scope='token-ip'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/report-submission-token")
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "192.0.2.1:12345".parse::<SocketAddr>().unwrap(),
    ));
    assert_eq!(
        app.router.clone().oneshot(request).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM report_submission_tokens")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "limited request must not create a challenge row");
}

#[tokio::test]
async fn parallel_identical_forms_save_one_gallery_and_notification_and_failed_insert_keeps_proof()
{
    let app = App::new().await;
    app.call(
        "PUT",
        "/api/v1/admin/report-settings",
        Some(json!({"recipients":["test@example.test"]})),
        true,
    )
    .await;
    let (first_token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    let (second_token, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    let photo = picture(ImageFormat::Jpeg, 4);
    let photos = [photo.as_slice()];
    let (first, second) = tokio::join!(
        submit_proof_photos(
            &app,
            &first_token,
            Some(&cookie),
            VALID,
            &photos,
            "192.0.2.1",
            false
        ),
        submit_proof_photos(
            &app,
            &second_token,
            Some(&cookie),
            VALID,
            &photos,
            "192.0.2.1",
            false
        ),
    );
    let statuses = [first.status(), second.status()];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::OK));
    assert_eq!(body_json(first).await, body_json(second).await);
    for query in [
        "SELECT count(*) FROM issue_reports",
        "SELECT count(*) FROM issue_report_photos",
        "SELECT count(*) FROM report_mail_queue",
        "SELECT count(*) FROM report_abuse_events",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "{query}");
    }
    let (token, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    sqlx::query(
        "ALTER TABLE report_mail_queue ADD CONSTRAINT test_reject_report_mail CHECK (report_id=1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let fields = [("description", "Jiná závada"), ("address", "Vyskeř")];
    assert_eq!(
        submit_proof_photos(
            &app,
            &token,
            Some(&cookie),
            &fields,
            &photos,
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let (claimed, payload): (Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT report_id,payload_hash FROM report_submission_tokens WHERE token_hash=$1",
    )
    .bind(backend::auth::hash(&token))
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(claimed.is_none() && payload.is_none());
    sqlx::query("ALTER TABLE report_mail_queue DROP CONSTRAINT test_reject_report_mail")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        submit_proof_photos(
            &app,
            &token,
            Some(&cookie),
            &fields,
            &photos,
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    for query in [
        "SELECT count(*) FROM issue_reports",
        "SELECT count(*) FROM issue_report_photos",
        "SELECT count(*) FROM report_mail_queue",
        "SELECT count(*) FROM report_abuse_events",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(count, 2, "{query}");
    }
}

async fn seed_quota_events(app: &App, count: i64, browser: &str, ip: &str, created_at: i64) {
    sqlx::query("WITH added AS (INSERT INTO issue_reports(description,address,request_ip,created_at) SELECT 'Quota fixture','Vyskeř','','2026-10-25T12:00:00Z' FROM generate_series(1,$1::bigint) RETURNING id) INSERT INTO report_abuse_events(report_id,browser_hash,ip_hash,created_at) SELECT id,$2,$3,$4 FROM added")
        .bind(count).bind(browser).bind(ip).bind(created_at).execute(&app.state.pool).await.unwrap();
}

#[tokio::test]
async fn rolling_day_and_shared_ip_quotas_are_enforced_independently() {
    let app = App::new().await;
    let (token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    assert_eq!(
        submit_proof(&app, &token, Some(&cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::CREATED
    );
    let (browser, ip): (String, String) =
        sqlx::query_as("SELECT browser_hash,ip_hash FROM report_abuse_events LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    seed_quota_events(&app, 49, &browser, "other-network", now - 3600).await;
    let fields = [("description", "Nová závada"), ("address", "Vyskeř")];
    let (token, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    assert_eq!(
        submit_proof(&app, &token, Some(&cookie), &fields, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    sqlx::query("UPDATE report_abuse_events SET created_at=$1")
        .bind(now - 86401)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        submit_proof(&app, &token, Some(&cookie), &fields, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::CREATED
    );
    sqlx::query("DELETE FROM report_abuse_events")
        .execute(&app.state.pool)
        .await
        .unwrap();
    seed_quota_events(&app, 100, "other-browser", &ip, now).await;
    let (token, other_cookie) = issue_token(&app, None, "192.0.2.1").await;
    assert_eq!(
        submit_proof(&app, &token, Some(&other_cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    sqlx::query("UPDATE report_abuse_events SET created_at=$1")
        .bind(now - 601)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        submit_proof(&app, &token, Some(&other_cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::CREATED
    );
    seed_quota_events(&app, 399, "yet-another-browser", &ip, now - 3600).await;
    let (token, third_cookie) = issue_token(&app, None, "192.0.2.1").await;
    assert_eq!(
        submit_proof(&app, &token, Some(&third_cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // The rolling day window expires without a restart or resetting the whole application.
    sqlx::query("UPDATE report_abuse_events SET created_at=$1")
        .bind(now - 86401)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        submit_proof(&app, &token, Some(&third_cookie), VALID, "192.0.2.1", false)
            .await
            .status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn abuse_migration_preserves_legacy_reports_with_thirty_day_grace_and_cleans_pending_mail() {
    let pool = common::test_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let before = OffsetDateTime::now_utc().unix_timestamp();
    let schema = format!("report_abuse_upgrade_{}", &backend::auth::token()[..32]);
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('search_path', $1, true)")
        .bind(&schema)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE navigation_items(label TEXT,path TEXT,sort_order BIGINT,visible BOOLEAN); CREATE TABLE administrators(id BIGINT PRIMARY KEY)")
        .execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0015_issue_reports.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0016_issue_report_photos.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/0017_issue_report_resolutions.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    let description =
        "Poškozená tabulka s nápisem:\nKdo nahlásil: tento text patří do popisu.\nOpravit prosím.";
    sqlx::query("INSERT INTO issue_reports(description,address,reporter_name,request_ip,created_at,resolution_comment) VALUES($1,'Vyskeř 50','Soukromé jméno','192.0.2.1','2026-10-25T12:00:00Z','Čeká na opravu')")
        .bind(description).execute(&mut *tx).await.unwrap();
    let old_body = format!(
        "Nové hlášení závady č. 1\nNahlášeno: 25. 10. 2026\nKdo nahlásil: Soukromé jméno\n\n{description}\n\nDetail hlášení: https://example.test/hlaseni-zavad/1\n"
    );
    sqlx::query("INSERT INTO report_mail_queue(report_id,recipient,subject,body,next_attempt_at,created_at) VALUES(1,'test@example.test','Nové hlášení',$1,0,0)")
        .bind(&old_body).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0018_report_abuse.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let (stored_description,name,ip,recorded,expires,resolution): (String,Option<String>,String,Option<i64>,i64,Option<String>) = sqlx::query_as("SELECT description,reporter_name,request_ip,metadata_recorded_at,metadata_expires_at,resolution_comment FROM issue_reports WHERE id=1")
        .fetch_one(&mut *tx).await.unwrap();
    assert_eq!(stored_description, description);
    assert_eq!(name.as_deref(), Some("Soukromé jméno"));
    assert_eq!(ip, "192.0.2.1");
    assert!(
        recorded.is_none(),
        "an upgrade must not invent a metadata capture date"
    );
    assert!(
        expires >= before + 30 * 86400 - 5
            && expires <= OffsetDateTime::now_utc().unix_timestamp() + 30 * 86400 + 1
    );
    assert_eq!(resolution.as_deref(), Some("Čeká na opravu"));
    let cleaned: String =
        sqlx::query_scalar("SELECT body FROM report_mail_queue WHERE report_id=1")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        cleaned,
        format!(
            "Nové hlášení závady č. 1\nNahlášeno: 25. 10. 2026\n\n{description}\n\nDetail hlášení: https://example.test/hlaseni-zavad/1\n"
        )
    );
    assert!(!cleaned.contains("Soukromé jméno"));
    assert!(cleaned.contains("Kdo nahlásil: tento text patří do popisu."));
    let contacts: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT reporter_email,reporter_phone FROM issue_reports WHERE id=1")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(contacts, (None, None));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn reporter_contacts_are_optional_validated_and_part_of_the_idempotent_payload() {
    let app = App::new().await;
    let (token, cookie) = issue_token(&app, None, "192.0.2.1").await;
    for (field, value) in [
        ("reporter_email", "neni-adresa".to_owned()),
        (
            "reporter_email",
            "kontakt@example.test\nCc: jiny@example.test".to_owned(),
        ),
        (
            "reporter_email",
            format!("{}@example.test", "x".repeat(255)),
        ),
        ("reporter_phone", "volejte starostovi".to_owned()),
        ("reporter_phone", "12".to_owned()),
        ("reporter_phone", "123456789012345678901".to_owned()),
        ("reporter_phone", "123\n456".to_owned()),
    ] {
        let fields = [
            ("description", "Rozbitá lavička"),
            ("address", "Vyskeř"),
            (field, value.as_str()),
        ];
        assert_eq!(
            submit_proof(&app, &token, Some(&cookie), &fields, "192.0.2.1", false)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{field}: {value:?}"
        );
    }
    let first_fields = [
        ("description", "Rozbitá lavička"),
        ("address", "Vyskeř"),
        ("reporter_email", " Kontakt@Example.test "),
        ("reporter_phone", "(+420) 123 456 789"),
    ];
    assert_eq!(
        submit_proof(
            &app,
            &token,
            Some(&cookie),
            &first_fields,
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let changed_fields = [
        ("description", "Rozbitá lavička"),
        ("address", "Vyskeř"),
        ("reporter_email", "jiny@example.test"),
        ("reporter_phone", "(+420) 123 456 789"),
    ];
    assert_eq!(
        submit_proof(
            &app,
            &token,
            Some(&cookie),
            &changed_fields,
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let (second_token, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    assert_eq!(
        submit_proof(
            &app,
            &second_token,
            Some(&cookie),
            &changed_fields,
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let phone_changed = [
        ("description", "Rozbitá lavička"),
        ("address", "Vyskeř"),
        ("reporter_email", "jiny@example.test"),
        ("reporter_phone", "123 456 780"),
    ];
    assert_eq!(
        submit_proof(
            &app,
            &second_token,
            Some(&cookie),
            &phone_changed,
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let (empty_token, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    let response = submit_proof(
        &app,
        &empty_token,
        Some(&cookie),
        &[
            ("description", "Rozbitá lavička"),
            ("address", "Vyskeř"),
            ("reporter_email", "  "),
            ("reporter_phone", ""),
        ],
        "192.0.2.1",
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = body_json(response).await["id"].as_i64().unwrap();
    let contacts: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT reporter_email,reporter_phone FROM issue_reports WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(contacts, (None, None));
    let (blank_retry, _) = issue_token(&app, Some(&cookie), "192.0.2.1").await;
    assert_eq!(
        submit_proof(
            &app,
            &blank_retry,
            Some(&cookie),
            &[("description", "Rozbitá lavička"), ("address", "Vyskeř")],
            "192.0.2.1",
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn public_status_selection_defaults_to_unresolved_and_matches_map_search_and_pagination() {
    let app = App::new().await;
    sqlx::query("INSERT INTO issue_reports(description,address,latitude,longitude,status,hidden,request_ip,created_at) VALUES ('Lampa sever','Vyskeř',50.5,15.1,'reported',FALSE,'','2026-10-25T12:00:00Z'),('Lampa jih','Vyskeř',50.5,15.1,'in_progress',FALSE,'','2026-10-25T12:00:00Z'),('Lampa u školy','Vyskeř',50.5,15.1,'resolved',FALSE,'','2026-10-25T12:00:00Z'),('Lampa bez polohy','Vyskeř',NULL,NULL,'reported',FALSE,'','2026-10-25T12:00:00Z'),('Lampa bez polohy v řešení','Vyskeř',NULL,NULL,'in_progress',FALSE,'','2026-10-25T12:00:00Z'),('Lampa bez polohy vyřešená','Vyskeř',NULL,NULL,'resolved',FALSE,'','2026-10-25T12:00:00Z'),('Lavička','U lampy',50.5,15.1,'reported',FALSE,'','2026-10-25T12:00:00Z'),('Lavička','Vyskeř',50.5,15.1,'in_progress',FALSE,'','2026-10-25T12:00:00Z'),('Lavička','Vyskeř',50.5,15.1,'resolved',FALSE,'','2026-10-25T12:00:00Z'),('Skrytá lampa','Vyskeř',50.5,15.1,'reported',TRUE,'','2026-10-25T12:00:00Z'),('Skrytá lampa v řešení','Vyskeř',NULL,NULL,'in_progress',TRUE,'','2026-10-25T12:00:00Z'),('Skrytá lampa vyřešená','Vyskeř',50.5,15.1,'resolved',TRUE,'','2026-10-25T12:00:00Z')")
        .execute(&app.state.pool).await.unwrap();
    let ids = |rows: &serde_json::Value| {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_i64().unwrap())
            .collect::<Vec<_>>()
    };
    for (query, expected, located, unlocated) in [
        ("", vec![8, 7, 5, 4, 2, 1], vec![8, 7, 2, 1], 2),
        (
            "?status=unresolved",
            vec![8, 7, 5, 4, 2, 1],
            vec![8, 7, 2, 1],
            2,
        ),
        (
            "?status=all",
            vec![9, 8, 7, 6, 5, 4, 3, 2, 1],
            vec![9, 8, 7, 3, 2, 1],
            3,
        ),
        ("?status=reported", vec![7, 4, 1], vec![7, 1], 1),
        ("?status=in_progress", vec![8, 5, 2], vec![8, 2], 1),
        ("?status=resolved", vec![9, 6, 3], vec![9, 3], 1),
        ("?q=LAMP", vec![7, 5, 4, 2, 1], vec![7, 2, 1], 2),
        (
            "?q=LAMP&status=all",
            vec![7, 6, 5, 4, 3, 2, 1],
            vec![7, 3, 2, 1],
            3,
        ),
        ("?q=LAMP&status=resolved", vec![6, 3], vec![3], 1),
        ("?q=missing&status=all", vec![], vec![], 0),
    ] {
        let list = app
            .call("GET", &format!("/api/v1/reports{query}"), None, false)
            .await;
        assert_eq!(list.status(), StatusCode::OK, "{query}");
        assert_eq!(ids(&body_json(list).await), expected, "{query}");
        let map = body_json(
            app.call("GET", &format!("/api/v1/reports/map{query}"), None, false)
                .await,
        )
        .await;
        assert_eq!(ids(&map["points"]), located, "{query}");
        assert_eq!(map["located_count"], located.len(), "{query}");
        assert_eq!(map["unlocated_count"], unlocated, "{query}");
        assert_eq!(map["truncated"], false, "{query}");
    }
    let list = body_json(
        app.call(
            "GET",
            "/api/v1/reports?q=LAMP&limit=2&offset=2",
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(
        ids(&list),
        vec![4, 2],
        "pagination must follow the default status predicate"
    );
    let list = body_json(
        app.call(
            "GET",
            "/api/v1/reports?q=LAMP&status=all&limit=2&offset=2",
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(ids(&list), vec![5, 4]);
    let map = body_json(
        app.call(
            "GET",
            "/api/v1/reports/map?q=LAMP&status=all&limit=2&offset=2",
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(ids(&map["points"]), vec![7, 3, 2, 1]);
    assert_eq!(map["unlocated_count"], 3);
    // SSR uses these same pool functions, with an explicitly typed selection.
    let selected =
        reports::public_reports_from_pool(&app.state.pool, 20, 0, "", ReportStatusFilter::Resolved)
            .await
            .unwrap();
    assert_eq!(
        selected.iter().map(|report| report.id).collect::<Vec<_>>(),
        vec![9, 6, 3]
    );
    let selected_map =
        reports::public_report_map_from_pool(&app.state.pool, "", ReportStatusFilter::Resolved)
            .await
            .unwrap();
    assert_eq!(
        selected_map
            .points
            .iter()
            .map(|report| report.id)
            .collect::<Vec<_>>(),
        vec![9, 3]
    );
    assert_eq!(
        app.call("GET", "/api/v1/reports/3", None, false)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call("GET", "/api/v1/reports/12", None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let admin = body_json(app.call("GET", "/api/v1/admin/reports", None, true).await).await;
    assert_eq!(
        ids(&admin),
        (1..=12).rev().collect::<Vec<_>>(),
        "administration keeps every status and hidden records"
    );
    for invalid in ["", "completed", "ALL", "in-progress", "%27%20OR%201=1--"] {
        for endpoint in ["/api/v1/reports", "/api/v1/reports/map"] {
            assert_eq!(
                app.call("GET", &format!("{endpoint}?status={invalid}"), None, false)
                    .await
                    .status(),
                StatusCode::BAD_REQUEST,
                "{endpoint}: {invalid}"
            );
        }
    }
}

#[tokio::test]
async fn report_map_status_selection_applies_before_point_cap_and_all_counts() {
    let app = App::new().await;
    sqlx::query("INSERT INTO issue_reports(description,address,latitude,longitude,status,request_ip,created_at) SELECT 'Limit mapy','Vyskeř',50.529,15.158,'resolved','','2026-10-25T12:00:00Z' FROM generate_series(1,$1::bigint)")
        .bind(REPORT_MAP_LIMIT+1).execute(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO issue_reports(description,address,latitude,longitude,status,request_ip,created_at) VALUES ('Limit mapy','Vyskeř',50.529,15.158,'reported','','2026-10-25T12:00:00Z'),('Limit mapy bez polohy','Vyskeř',NULL,NULL,'reported','','2026-10-25T12:00:00Z')")
        .execute(&app.state.pool).await.unwrap();
    for (status, located, unlocated, points, truncated) in [
        ("unresolved", 1, 1, 1, false),
        ("reported", 1, 1, 1, false),
        ("in_progress", 0, 0, 0, false),
        ("resolved", REPORT_MAP_LIMIT + 1, 0, REPORT_MAP_LIMIT, true),
        ("all", REPORT_MAP_LIMIT + 2, 1, REPORT_MAP_LIMIT, true),
    ] {
        let map = body_json(
            app.call(
                "GET",
                &format!("/api/v1/reports/map?q=limit&status={status}"),
                None,
                false,
            )
            .await,
        )
        .await;
        assert_eq!(map["located_count"], located, "{status}");
        assert_eq!(map["unlocated_count"], unlocated, "{status}");
        assert_eq!(
            map["points"].as_array().unwrap().len(),
            points as usize,
            "{status}"
        );
        assert_eq!(map["truncated"], truncated, "{status}");
    }
    let default = body_json(
        app.call("GET", "/api/v1/reports/map?q=limit", None, false)
            .await,
    )
    .await;
    assert_eq!(default["located_count"], 1);
    assert_eq!(default["unlocated_count"], 1);
    assert_eq!(default["truncated"], false);
}

#[tokio::test]
async fn report_section_rename_only_changes_the_old_default_link_and_preserves_customization() {
    let pool = common::test_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("report_name_upgrade_{}", &backend::auth::token()[..32]);
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('search_path', $1, true)")
        .bind(&schema)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE navigation_items(id BIGINT PRIMARY KEY,label TEXT,path TEXT,sort_order BIGINT,visible BOOLEAN,version BIGINT,parent_id BIGINT)")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO navigation_items(id,label,path,sort_order,visible,version,parent_id) VALUES (1,'Hlášení závad','/hlaseni-zavad',73,FALSE,4,NULL),(2,'Místní podněty','/hlaseni-zavad',80,TRUE,7,1),(3,'Hlášení závad','/jiny-prehled',90,TRUE,8,NULL),(4,'Nahlášené závady','/hlaseni-zavad',100,TRUE,5,NULL),(5,'Podněty a závady','/hlaseni-zavad',110,TRUE,2,NULL),(6,'Hlášení závad','/hlaseni-zavad/nove',120,TRUE,3,1)")
        .execute(&mut *tx).await.unwrap();
    type NavigationRow = (i64, String, String, i64, bool, i64, Option<i64>);
    let before: Vec<NavigationRow> = sqlx::query_as("SELECT id,label,path,sort_order,visible,version,parent_id FROM navigation_items ORDER BY id")
        .fetch_all(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0019_report_section_name.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let renamed: Vec<NavigationRow> = sqlx::query_as("SELECT id,label,path,sort_order,visible,version,parent_id FROM navigation_items ORDER BY id")
        .fetch_all(&mut *tx).await.unwrap();
    let mut expected = before;
    expected[0].1 = "Podněty a závady".into();
    expected[0].5 += 1;
    assert_eq!(renamed, expected);
    sqlx::raw_sql(include_str!("../migrations/0019_report_section_name.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let repeated: Vec<NavigationRow> = sqlx::query_as("SELECT id,label,path,sort_order,visible,version,parent_id FROM navigation_items ORDER BY id")
        .fetch_all(&mut *tx).await.unwrap();
    assert_eq!(
        repeated, expected,
        "reapplying a rename must not overwrite custom titles or advance versions again"
    );
    tx.rollback().await.unwrap();
}
