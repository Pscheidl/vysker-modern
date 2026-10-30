#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, body_json};
use http_body_util::BodyExt;
use image::{DynamicImage, ImageFormat, ImageReader};
use serde_json::{Value, json};
use std::io::Cursor;
use tower::ServiceExt;

async fn report(app: &App) -> i64 {
    sqlx::query_scalar("INSERT INTO issue_reports(description,address,request_ip,created_at,resolution_comment) VALUES ('Rozbitá lavička','Vyskeř 50','local','2026-10-30T12:00:00+01:00','Starší vyjádření obce') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap()
}

fn picture() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(3000, 2)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

async fn submit(
    app: &App,
    method: &str,
    path: &str,
    fields: &[(&str, &str)],
    photos: &[&[u8]],
    authenticated: bool,
    csrf: bool,
) -> Response {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--comment-boundary\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    for photo in photos {
        body.extend_from_slice(b"--comment-boundary\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"foto.png\"\r\nContent-Type: image/png\r\n\r\n");
        body.extend_from_slice(photo);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--comment-boundary--\r\n");
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(
            "content-type",
            "multipart/form-data; boundary=comment-boundary",
        )
        .header("origin", "http://localhost:3000");
    if authenticated {
        request = request.header("cookie", &app.cookie);
    }
    if csrf {
        request = request.header("x-csrf-token", &app.csrf);
    }
    app.router
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn create(app: &App, report_id: i64, name: &str, body: &str, photos: &[&[u8]]) -> Value {
    let response = submit(
        app,
        "POST",
        &format!("/api/v1/admin/reports/{report_id}/comments"),
        &[("author_name", name), ("body", body)],
        photos,
        true,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await
}

#[tokio::test]
async fn signed_comments_keep_history_sanitize_photos_and_follow_report_visibility() {
    let app = App::new().await;
    let id = report(&app).await;
    let png = picture();
    let first = create(
        &app,
        id,
        "  Jana Nováková  ",
        "  Oprava objednána.\r\nDěkujeme.  ",
        &[&png],
    )
    .await;
    assert_eq!(first["author_name"], "Jana Nováková");
    assert_eq!(first["body"], "Oprava objednána.\nDěkujeme.");
    assert_eq!(first["created_at"], first["updated_at"]);
    let second = create(&app, id, "Petr Svoboda", "Oprava dokončena.", &[]).await;
    let comments = body_json(
        app.call(
            "GET",
            &format!("/api/v1/reports/{id}/comments"),
            None,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(comments.as_array().unwrap(), &vec![first.clone(), second]);
    assert!(comments[0].get("administrator_id").is_none());
    assert!(comments[0].get("email").is_none());
    let cid = first["id"].as_i64().unwrap();
    let photo_id = first["photo_ids"][0].as_i64().unwrap();
    let photo_path = format!("/api/v1/reports/{id}/comments/{cid}/photos/{photo_id}");
    let response = app.call("GET", &photo_path, None, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/jpeg");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(image.width(), 2560);
    let creator: i64 =
        sqlx::query_scalar("SELECT administrator_id FROM issue_report_comments WHERE id=$1")
            .bind(cid)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(creator, app.admin_id);
    let old: String =
        sqlx::query_scalar("SELECT resolution_comment FROM issue_reports WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(old, "Starší vyjádření obce");
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/reports/{id}"),
            Some(json!({"hidden":true})),
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/reports/{id}/comments"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call("GET", &photo_path, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/reports/{id}/comments"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/reports/{id}/comments/{cid}/photos/{photo_id}"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn editing_and_deletion_are_atomic_audited_and_reject_stale_versions() {
    let app = App::new().await;
    let id = report(&app).await;
    let other_id = report(&app).await;
    let png = picture();
    let original = create(&app, id, "Jana", "Objednáno.", &[&png]).await;
    let other = create(&app, other_id, "Petr", "Jiný podnět.", &[&png]).await;
    let cid = original["id"].as_i64().unwrap();
    let path = format!("/api/v1/admin/reports/{id}/comments/{cid}");
    let old_version = original["updated_at"].as_str().unwrap();
    let wrong_remove = json!([other["photo_ids"][0]]).to_string();
    let response = submit(
        &app,
        "PUT",
        &path,
        &[
            ("author_name", "Jana"),
            ("body", "Nesmí se uložit."),
            ("expected_updated_at", old_version),
            ("remove_photo_ids", &wrong_remove),
        ],
        &[],
        true,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let remove = original["photo_ids"].to_string();
    let response = submit(
        &app,
        "PUT",
        &path,
        &[
            ("author_name", "Jana Nováková"),
            ("body", "Opraveno."),
            ("expected_updated_at", old_version),
            ("remove_photo_ids", &remove),
        ],
        &[&png],
        true,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let edited = body_json(response).await;
    assert_eq!(edited["body"], "Opraveno.");
    assert_eq!(edited["created_at"], original["created_at"]);
    assert_ne!(edited["updated_at"], original["updated_at"]);
    assert_ne!(edited["photo_ids"], original["photo_ids"]);
    let removed_photo_id = original["photo_ids"][0].as_i64().unwrap();
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/reports/{id}/comments/{cid}/photos/{removed_photo_id}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let stale = submit(
        &app,
        "PUT",
        &path,
        &[
            ("author_name", "Jana"),
            ("body", "Stará verze."),
            ("expected_updated_at", old_version),
        ],
        &[],
        true,
        true,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        app.call(
            "DELETE",
            &path,
            Some(json!({"expected_updated_at":old_version})),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "DELETE",
            &path,
            Some(json!({"expected_updated_at":edited["updated_at"]})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let photos: i64 =
        sqlx::query_scalar("SELECT count(*) FROM issue_report_comment_photos WHERE comment_id=$1")
            .bind(cid)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(photos, 0);
    let ops: Vec<String> = sqlx::query_scalar("SELECT operation FROM audit_log WHERE entity_type='issue_report_comment' AND entity_id=$1 ORDER BY id")
        .bind(cid).fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(ops, vec!["created", "updated", "deleted"]);
    assert_eq!(
        body_json(
            app.call(
                "GET",
                &format!("/api/v1/reports/{id}/comments"),
                None,
                false
            )
            .await
        )
        .await,
        json!([])
    );
}

#[tokio::test]
async fn writes_require_authentication_csrf_name_body_and_valid_images() {
    let app = App::new().await;
    let id = report(&app).await;
    let path = format!("/api/v1/admin/reports/{id}/comments");
    let fields = [("author_name", "Jana"), ("body", "Děkujeme.")];
    assert_eq!(
        submit(&app, "POST", &path, &fields, &[], false, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        submit(&app, "POST", &path, &fields, &[], true, false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let long_name = "ž".repeat(121);
    let long_body = "ž".repeat(5001);
    for invalid in [
        vec![("body", "Text")],
        vec![("author_name", " "), ("body", "Text")],
        vec![("author_name", "Jana")],
        vec![("author_name", "Jana"), ("body", " ")],
        vec![("author_name", long_name.as_str()), ("body", "Text")],
        vec![("author_name", "Jana"), ("body", long_body.as_str())],
        vec![("author_name", "Jana\nNováková"), ("body", "Text")],
        vec![
            ("author_name", "Jana"),
            ("body", "Text"),
            ("body", "Duplicitní"),
        ],
        vec![
            ("author_name", "Jana"),
            ("body", "Text"),
            ("administrator_id", "1"),
        ],
    ] {
        assert_eq!(
            submit(&app, "POST", &path, &invalid, &[], true, true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        submit(&app, "POST", &path, &fields, &[b"not an image"], true, true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let comments: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_report_comments")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(comments, 0);
    assert_eq!(
        submit(
            &app,
            "POST",
            "/api/v1/admin/reports/999999/comments",
            &fields,
            &[],
            true,
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let comment = create(&app, id, "Jana", "Chráněný komentář.", &[]).await;
    let comment_path = format!("{path}/{}", comment["id"].as_i64().unwrap());
    let version = comment["updated_at"].as_str().unwrap();
    let edit_fields = [
        ("author_name", "Jana"),
        ("body", "Nesmí se uložit."),
        ("expected_updated_at", version),
    ];
    assert_eq!(
        submit(&app, "PUT", &comment_path, &edit_fields, &[], false, false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        submit(&app, "PUT", &comment_path, &edit_fields, &[], true, false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.call(
            "DELETE",
            &comment_path,
            Some(json!({"expected_updated_at":version})),
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&comment_path)
                .header("cookie", &app.cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"expected_updated_at":version}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let stored: String = sqlx::query_scalar("SELECT body FROM issue_report_comments WHERE id=$1")
        .bind(comment["id"].as_i64().unwrap())
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, "Chráněný komentář.");
}

#[tokio::test]
async fn comment_and_photo_ids_are_scoped_to_their_report_and_creator_is_preserved() {
    let app = App::new().await;
    let id = report(&app).await;
    let other_id = report(&app).await;
    let png = picture();
    let comment = create(&app, id, "Původní autor", "Původní text.", &[&png]).await;
    let cid = comment["id"].as_i64().unwrap();
    let photo_id = comment["photo_ids"][0].as_i64().unwrap();
    let version = comment["updated_at"].as_str().unwrap();
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/reports/{other_id}/comments/{cid}/photos/{photo_id}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let wrong_path = format!("/api/v1/admin/reports/{other_id}/comments/{cid}");
    assert_eq!(
        submit(
            &app,
            "PUT",
            &wrong_path,
            &[
                ("author_name", "Autor"),
                ("body", "Změna"),
                ("expected_updated_at", version)
            ],
            &[],
            true,
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "DELETE",
            &wrong_path,
            Some(json!({"expected_updated_at":version})),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let second_id = obecni_web::backend::auth::create_admin(
        &app.state,
        "second@vysker.test",
        "Druhe-testovaci-heslo-123456!".into(),
    )
    .await
    .unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"second@vysker.test","password":"Druhe-testovaci-heslo-123456!"})),
            false,
        )
        .await;
    assert_eq!(login.status(), StatusCode::OK);
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
    let second = App {
        state: app.state.clone(),
        router: app.router.clone(),
        cookie,
        csrf,
        admin_id: second_id,
    };
    let response = submit(
        &second,
        "PUT",
        &format!("/api/v1/admin/reports/{id}/comments/{cid}"),
        &[
            ("author_name", "Původní autor"),
            ("body", "Opravený text."),
            ("expected_updated_at", version),
        ],
        &[],
        true,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let creator: i64 =
        sqlx::query_scalar("SELECT administrator_id FROM issue_report_comments WHERE id=$1")
            .bind(cid)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(creator, app.admin_id);
    let editor: i64 = sqlx::query_scalar("SELECT actor_id FROM audit_log WHERE entity_type='issue_report_comment' AND entity_id=$1 AND operation='updated'").bind(cid).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(editor, second_id);
}
