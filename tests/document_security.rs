#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, PDF, body_json};
use http_body_util::BodyExt;
use obecni_web::{backend::legacy, catalog};
use serde_json::json;
use tower::ServiceExt;

const OLD_FILE: &str = "/assets/File.ashx?id_dokumenty=701&id_org=18774";
const OLD_IMAGE: &str = "/assets/File.ashx?id_dokumenty=702&id_org=18774";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

async fn public(app: &App, method: &str, path: &str) -> Response {
    app.router
        .clone()
        .layer(axum::middleware::from_fn_with_state(
            app.state.clone(),
            legacy::redirects,
        ))
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn source(app: &App, document: i64, attachment: i64, path: &str) {
    sqlx::query("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,attachment_id,destination) VALUES ($1,$2,repeat('a',64),'2020-01-01T00:00:00Z','2026-11-01T00:00:00Z','{}',$3,$4,$5)")
        .bind(path).bind(format!("https://vysker.cz{path}")).bind(document).bind(attachment)
        .bind(format!("/api/v1/attachments/{attachment}"))
        .execute(&app.state.pool).await.unwrap();
}

async fn fixture(app: &App) -> (i64, i64, i64) {
    let id = app
        .notice(
            json!({"title":"Bezpečnostní test dokumentu","description":"SoukromyKontrolniText"}),
        )
        .await;
    let uploaded = app
        .upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "kontrola.pdf",
            PDF,
        )
        .await;
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let pdf = body_json(uploaded).await["id"].as_i64().unwrap();
    let image: i64 = sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data,sort_order) VALUES ($1,'kontrola.png','image/png',8,$2,1) RETURNING id")
        .bind(id).bind(PNG).fetch_one(&app.state.pool).await.unwrap();
    source(app, id, pdf, OLD_FILE).await;
    source(app, id, image, OLD_IMAGE).await;
    sqlx::query(
        "INSERT INTO document_legacy_ids(old_model,old_id,document_id) VALUES ('notice',7000,$1)",
    )
    .bind(id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    (id, pdf, image)
}

async fn assert_denied_file(app: &App, file: i64, old: &str) {
    let direct = format!("/api/v1/attachments/{file}");
    for method in ["GET", "HEAD"] {
        let response = public(app, method, &direct).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{method} {direct}"
        );
        assert!(
            response.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("no-store")
        );
        let old_response = public(app, method, old).await;
        if old_response.status().is_redirection() {
            let target = old_response.headers()["location"].to_str().unwrap();
            assert_eq!(target, direct);
            assert_eq!(
                public(app, method, target).await.status(),
                StatusCode::NOT_FOUND
            );
        } else {
            assert_eq!(old_response.status(), StatusCode::NOT_FOUND);
        }
    }
    // An administrative session does not bypass access on a public URL.
    assert_eq!(
        app.call("GET", &direct, None, true).await.status(),
        StatusCode::NOT_FOUND
    );
    let admin = format!("/api/v1/admin/attachments/{file}");
    assert_eq!(
        app.call("GET", &admin, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let retained = app.call("GET", &admin, None, true).await;
    assert_eq!(retained.status(), StatusCode::OK);
    assert!(
        retained.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    assert!(
        !retained
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
}

#[tokio::test]
async fn whole_withdrawal_denies_old_aliases_files_media_ssr_and_stale_search_index() {
    let app = App::new().await;
    let (id, pdf, image) = fixture(&app).await;
    assert_eq!(
        public(&app, "GET", OLD_FILE).await.status(),
        StatusCode::MOVED_PERMANENTLY
    );
    assert_eq!(
        public(&app, "GET", &format!("/api/v1/attachments/{pdf}"))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        public(&app, "GET", &format!("/api/v1/legacy-media/{image}"))
            .await
            .status(),
        StatusCode::OK
    );
    let indexed: String = sqlx::query_scalar("SELECT search_text FROM documents WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/visibility"),
            Some(json!({"public_access":false,"reason":"Test okamžitého stažení"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT search_text FROM documents WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        indexed
    );
    assert_denied_file(&app, pdf, OLD_FILE).await;
    assert_denied_file(&app, image, OLD_IMAGE).await;
    for path in [
        format!("/api/v1/documents/{id}"),
        format!("/api/v1/notices/{id}"),
        format!("/api/v1/legacy-media/{image}"),
        "/uredni-deska/7000".into(),
    ] {
        assert_eq!(
            public(&app, "GET", &path).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    for path in [
        "/api/v1/documents",
        "/api/v1/notices",
        "/api/v1/notices?archived=true",
    ] {
        assert_eq!(body_json(public(&app, "GET", path).await).await, json!([]));
    }
    for kind in ["all", "document", "notice"] {
        let result = body_json(
            public(
                &app,
                "GET",
                &format!("/api/v1/search?kind={kind}&state=all&q=SoukromyKontrolniText"),
            )
            .await,
        )
        .await;
        assert_eq!(result["total"], 0);
    }
    assert!(
        catalog::document_from_pool(&app.state.pool, id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT lock_state FROM documents WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        "locked"
    );
}

#[tokio::test]
async fn private_archive_keeps_public_metadata_but_never_source_file_links() {
    let app = App::new().await;
    let (id, pdf, image) = fixture(&app).await;
    assert!(
        catalog::document_from_pool(&app.state.pool, id)
            .await
            .unwrap()
            .unwrap()
            .import_origin
            .unwrap()
            .source_url
            .is_some()
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/notices/{id}/withdraw"),
            Some(json!({"reason":"Test sejmutí"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let detail = catalog::document_from_pool(&app.state.pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.title, "Bezpečnostní test dokumentu");
    assert!(detail.files.is_empty());
    assert!(detail.import_origin.unwrap().source_url.is_none());
    assert_denied_file(&app, pdf, OLD_FILE).await;
    assert_denied_file(&app, image, OLD_IMAGE).await;
    assert_eq!(
        public(&app, "GET", &format!("/api/v1/legacy-media/{image}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/legacy-media/{image}"),
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
            &format!("/api/v1/admin/legacy-media/{image}"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    let stored: Vec<u8> = sqlx::query_scalar("SELECT data FROM attachments WHERE id=$1")
        .bind(pdf)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, PDF);
}

#[tokio::test]
async fn individually_private_attachment_never_exposes_its_source_url() {
    let app = App::new().await;
    let id = app.notice(json!({})).await;
    let file:i64=sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data,is_public) VALUES ($1,'never-public.pdf','application/pdf',$2,$3,FALSE) RETURNING id")
        .bind(id).bind(PDF.len() as i64).bind(PDF).fetch_one(&app.state.pool).await.unwrap();
    source(&app, id, file, OLD_FILE).await;
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    let detail = catalog::document_from_pool(&app.state.pool, id)
        .await
        .unwrap()
        .unwrap();
    assert!(detail.files.is_empty());
    assert!(detail.import_origin.unwrap().source_url.is_none());
    assert_denied_file(&app, file, OLD_FILE).await;
}

#[tokio::test]
async fn uncertain_removed_history_is_not_a_public_board_archive_entry() {
    let app = App::new().await;
    let id:i64=sqlx::query_scalar("INSERT INTO documents(title,description,status,created_at,public_access,board_status) VALUES ('Neověřená historie','Původní veřejný katalog','published','2020-01-01T00:00:00Z',TRUE,'archived') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("UPDATE documents SET lock_state='review' WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO document_postings(document_id,origin,certainty,removed_on,after_action,snapshot_sha256) VALUES ($1,'migration','uncertain','2020-01-01','hide',document_fingerprint($1))")
        .bind(id).execute(&app.state.pool).await.unwrap();
    assert_eq!(
        body_json(
            app.call("GET", "/api/v1/notices?archived=true", None, false)
                .await
        )
        .await,
        json!([])
    );
    let result = body_json(
        app.call(
            "GET",
            "/api/v1/search?kind=notice&state=archive",
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(result["total"], 0);
}

#[tokio::test]
async fn registry_and_revision_routes_reject_anonymous_readers_and_writers() {
    let app = App::new().await;
    let (id, file, image) = fixture(&app).await;
    for path in [
        format!("/api/v1/admin/documents/{id}"),
        format!("/api/v1/admin/documents/{id}/preview"),
        format!("/api/v1/admin/documents/{id}/history"),
        format!("/api/v1/admin/attachments/{file}"),
        format!("/api/v1/admin/legacy-media/{image}"),
        "/api/v1/admin/import-revisions".into(),
        "/api/v1/admin/import-revisions/1/files/0".into(),
    ] {
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    for path in [
        format!("/api/v1/admin/documents/{id}/visibility"),
        format!("/api/v1/admin/documents/{id}/postings"),
        format!("/api/v1/admin/documents/{id}/topics"),
        format!("/api/v1/admin/documents/{id}/corrections"),
        format!("/api/v1/admin/documents/{id}/relations"),
        format!("/api/v1/admin/documents/{id}/event-corrections"),
        "/api/v1/admin/postings/1/withdraw".into(),
        "/api/v1/admin/postings/1/plan".into(),
        "/api/v1/admin/import-revisions/1/decision".into(),
    ] {
        assert_eq!(
            app.call("POST", &path, Some(json!({})), false)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
}

#[tokio::test]
async fn deleted_draft_attachment_metadata_is_not_published_with_the_document() {
    let app = App::new().await;
    let id = app.notice(json!({})).await;
    let response = app
        .upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "private-draft-only.pdf",
            PDF,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let file = body_json(response).await["id"].as_i64().unwrap();
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/admin/attachments/{file}"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    for path in [
        format!("/api/v1/documents/{id}"),
        format!("/api/v1/notices/{id}"),
    ] {
        let detail = body_json(app.call("GET", &path, None, false).await).await;
        assert_eq!(detail["attachments"], json!([]), "{path}");
        assert!(!detail.to_string().contains("private-draft-only"));
    }
    assert!(
        catalog::document_from_pool(&app.state.pool, id)
            .await
            .unwrap()
            .unwrap()
            .files
            .is_empty()
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let retained_name: String = sqlx::query_scalar("SELECT name FROM attachments WHERE id=$1")
        .bind(file)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(retained_name, "private-draft-only.pdf");
}
