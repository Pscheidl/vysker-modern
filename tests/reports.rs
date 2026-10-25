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
use obecni_web::reports::{REPORT_MAP_LIMIT, REPORT_PHOTO_BYTES, REPORT_UPLOAD_BYTES, Report};
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
    let bytes = multipart_body(fields, photos);
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/reports")
        .header(
            "content-type",
            "multipart/form-data; boundary=report-boundary",
        )
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
    assert!(report["reporter_name"].is_null());
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
        assert_eq!(report["reporter_name"], "Jana");
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
async fn submission_rate_limit_uses_request_peer_and_recipient_snapshots_are_atomic() {
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
        submit(&app, VALID, None, Some(ORIGIN), "192.0.2.1")
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
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
    assert!(pending[0].1.contains("Anonymně"));
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
