#![cfg(feature = "ssr")]
mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{App, body_json};
use http_body_util::BodyExt;
use image::{DynamicImage, ImageFormat};
use serde_json::json;
use std::io::Cursor;
use tower::ServiceExt;

fn picture(format: ImageFormat, width: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(width, 2)
        .write_to(&mut bytes, format)
        .unwrap();
    bytes.into_inner()
}

async fn page(app: &App) -> i64 {
    let response = app
        .call(
            "POST",
            "/api/v1/admin/pages",
            Some(json!({"title":"Obrázky","slug":"obrazky","content":"Koncept","published":false})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await["id"].as_i64().unwrap()
}

async fn save(app: &App, id: i64, version: i64, published: bool, content: &str) {
    assert_eq!(app.call("PUT", &format!("/api/v1/admin/pages/{id}"), Some(json!({"title":"Obrázky","slug":"obrazky","content":content,"published":published,"expected_version":version})), true).await.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn images_follow_saved_public_content_and_survive_revision_restore() {
    let app = App::new().await;
    let page = page(&app).await;
    let upload_path = format!("/api/v1/admin/pages/{page}/images");
    let bytes = picture(ImageFormat::Png, 2);
    let response = app.upload(&upload_path, "kaple.png", &bytes).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let image = body_json(response).await;
    assert_eq!(image["width"], 2);
    assert_eq!(image["height"], 2);
    let id = image["id"].as_i64().unwrap();
    let url = format!("/api/v1/page-images/{id}");
    let markdown = format!("## Kaple\n\n![Kaple]({url})");
    assert_eq!(
        app.call("GET", &url, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    let preview = app.call("GET", &url, None, true).await;
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(preview.headers()["content-type"], "image/png");
    assert_eq!(preview.headers()["cache-control"], "no-store");
    assert_eq!(preview.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        preview
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        bytes
    );
    assert_eq!(
        app.call("GET", &upload_path, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    save(
        &app,
        page,
        1,
        true,
        &format!("```markdown\n{markdown}\n```"),
    )
    .await;
    assert_eq!(
        app.call("GET", &url, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    save(&app, page, 2, true, &markdown).await;
    assert_eq!(
        app.call("GET", &url, None, false).await.status(),
        StatusCode::OK
    );
    save(&app, page, 3, false, &markdown).await;
    assert_eq!(
        app.call("GET", &url, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    save(&app, page, 4, true, "Bez obrázku").await;
    assert_eq!(
        app.call("GET", &url, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    let history = body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/pages/{page}/revisions"),
            None,
            true,
        )
        .await,
    )
    .await;
    let old = history
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["version"] == 3)
        .unwrap();
    save(&app, page, 5, true, old["content"].as_str().unwrap()).await;
    assert_eq!(
        app.call("GET", &url, None, false).await.status(),
        StatusCode::OK
    );
    let library = body_json(app.call("GET", &upload_path, None, true).await).await;
    assert_eq!(library.as_array().unwrap().len(), 1);
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE entity_type='page_image' AND entity_id=$1 AND operation='uploaded'").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn image_types_are_detected_from_decoded_contents() {
    let app = App::new().await;
    let id = page(&app).await;
    let path = format!("/api/v1/admin/pages/{id}/images");
    for (format, mime) in [
        (ImageFormat::Png, "image/png"),
        (ImageFormat::Jpeg, "image/jpeg"),
        (ImageFormat::WebP, "image/webp"),
        (ImageFormat::Gif, "image/gif"),
    ] {
        let response = app.upload(&path, "obrázek.bin", &picture(format, 2)).await;
        assert_eq!(response.status(), StatusCode::CREATED, "{format:?}");
        let id = body_json(response).await["id"].as_i64().unwrap();
        let response = app
            .call("GET", &format!("/api/v1/page-images/{id}"), None, true)
            .await;
        assert_eq!(response.headers()["content-type"], mime);
    }
    for bytes in [
        b"<svg onload='alert(1)'></svg>".to_vec(),
        b"\x89PNG\r\n\x1a\ntruncated".to_vec(),
        vec![],
        picture(ImageFormat::Png, 8193),
        vec![0; obecni_web::backend::MAX_FILE_BYTES + 1],
    ] {
        assert_eq!(
            app.upload(&path, "fake.png", &bytes).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        app.upload(&path, "../picture.png", &picture(ImageFormat::Png, 2))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.upload(
            "/api/v1/admin/pages/999999/images",
            "picture.png",
            &picture(ImageFormat::Png, 2)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM page_images")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 4);
}

#[tokio::test]
async fn uploads_require_session_csrf_and_same_origin() {
    let app = App::new().await;
    let id = page(&app).await;
    let path = format!("/api/v1/admin/pages/{id}/images");
    for (cookie, csrf, origin, status) in [
        ("", "", "http://localhost:3000", StatusCode::UNAUTHORIZED),
        (
            app.cookie.as_str(),
            "",
            "http://localhost:3000",
            StatusCode::FORBIDDEN,
        ),
        (
            app.cookie.as_str(),
            app.csrf.as_str(),
            "https://external.test",
            StatusCode::FORBIDDEN,
        ),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header("origin", origin)
                    .header("content-type", "multipart/form-data; boundary=test")
                    .body(Body::from("--test--\r\n"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM page_images")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn imported_images_appear_once_in_their_page_library_and_have_private_previews() {
    let app = App::new().await;
    let page = page(&app).await;
    let document: i64 = sqlx::query_scalar("INSERT INTO documents(title,status,created_at) VALUES ('Imported photos','draft','now') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    let bytes = picture(ImageFormat::Png, 2);
    let mut imported = Vec::new();
    for name in [
        "used.png",
        "code-example.png",
        "plain-link.png",
        "unverified.png",
    ] {
        let id: i64 = sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES ($1,$2,'image/png',$3,$4) RETURNING id")
            .bind(document).bind(name).bind(bytes.len() as i64).bind(&bytes).fetch_one(&app.state.pool).await.unwrap();
        if name != "unverified.png" {
            sqlx::query("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,attachment_id,destination) VALUES ($1,'https://vysker.cz/assets/Image.ashx','hash','now','now','{}',$2,$3,$4)")
                .bind(name).bind(document).bind(id).bind(format!("/api/v1/attachments/{id}"))
                .execute(&app.state.pool).await.unwrap();
        }
        imported.push(id);
    }
    let public = format!("/api/v1/legacy-media/{}", imported[0]);
    let preview = format!("/api/v1/admin/legacy-media/{}", imported[0]);
    save(&app, page, 1, false, &format!("![Photo]({public})\n\n![Again][photo]\n\n[photo]: {public}\n\n`![Code](/api/v1/legacy-media/{})`\n\n[Download](/api/v1/legacy-media/{})\n\n![Unverified](/api/v1/legacy-media/{})", imported[1], imported[2], imported[3])).await;
    let path = format!("/api/v1/admin/pages/{page}/images");
    let images = body_json(app.call("GET", &path, None, true).await).await;
    assert_eq!(images.as_array().unwrap().len(), 1);
    assert_eq!(images[0]["url"], public);
    assert_eq!(images[0]["preview_url"], preview);
    assert!(images[0]["width"].is_null());
    assert_eq!(
        app.call("GET", &preview, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call("GET", &public, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    let response = app.call("GET", &preview, None, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        bytes
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/legacy-media/{}", imported[3]),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    // An uploaded image and a legacy attachment can share a numeric id.
    let uploaded = app.upload(&path, "new.png", &bytes).await;
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let uploaded = body_json(uploaded).await;
    assert_ne!(uploaded["url"], images[0]["url"]);
    for version in 2..=22 {
        save(
            &app,
            page,
            version,
            false,
            "Photograph removed from current text",
        )
        .await;
    }
    let images = body_json(app.call("GET", &path, None, true).await).await;
    assert_eq!(images.as_array().unwrap().len(), 2);
    assert!(
        images
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["url"] == public)
    );
    sqlx::query("UPDATE attachments SET data=NULL,removed_at='now' WHERE id=$1")
        .bind(imported[0])
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", &preview, None, true).await.status(),
        StatusCode::NOT_FOUND
    );
    let images = body_json(app.call("GET", &path, None, true).await).await;
    assert_eq!(images.as_array().unwrap().len(), 1);
    assert_eq!(images[0]["url"], uploaded["url"]);
}
