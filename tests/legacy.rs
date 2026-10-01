#![cfg(feature = "ssr")]
mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware,
};
use obecni_web::backend::legacy;
use tower::ServiceExt;

#[test]
fn legacy_urls_have_stable_identity_without_unsafe_actions() {
    for path in ["/nazev%2Dclanku/d-1020/p1=53", "/jiny-nazev/d-1020"] {
        assert_eq!(legacy::source_key(path, None).as_deref(), Some("d:1020"));
    }
    assert_eq!(
        legacy::source_key("/vismo/dokumenty2.asp", Some("id_org=18774&id=1020&n=old")).as_deref(),
        Some("d:1020")
    );
    assert_eq!(
        legacy::source_key("/assets/File.ashx", Some("id_org=18774&id_dokumenty=123")).as_deref(),
        Some("/assets/File.ashx?id_dokumenty=123&id_org=18774")
    );
    assert_eq!(
        legacy::source_key("/vismo/dokumenty2.asp", Some("id=1&xvvolba1=2")),
        None
    );
    assert_eq!(legacy::source_key("/kontakt", None), None);
    assert_eq!(
        legacy::source_key("/fotografie/g-1051/id_obrazky=1081%26typ_sady=1", None).as_deref(),
        Some("g:1051/id_obrazky=1081%26typ_sady=1")
    );
    assert_eq!(
        legacy::source_key("/dsp/archiv=1", None).as_deref(),
        Some("/dsp/archiv=1")
    );
    assert_eq!(
        legacy::source_key("/ap/kdy=5%26hledani=1%26misto=10", None).as_deref(),
        Some("/ap/kdy=5%26hledani=1%26misto=10")
    );
    assert_eq!(legacy::source_key("/api/v1/admin/accounts", None), None);
}

#[tokio::test]
async fn redirects_and_images_follow_publication_and_withdrawal() {
    let app = common::App::new().await;
    let page:i64=sqlx::query_scalar("INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('legacy','Old','Body',FALSE,'now') RETURNING id").fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,page_id,destination) VALUES ('ms:123','https://vysker.cz/old/ms-123','hash','now','now','{}',$1,'/stranky/legacy')").bind(page).execute(&app.state.pool).await.unwrap();
    let router = app.router.clone().layer(middleware::from_fn_with_state(
        app.state.clone(),
        legacy::redirects,
    ));
    let get = || {
        Request::builder()
            .uri("/old/ms-123/p1=55")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        router.clone().oneshot(get()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE pages SET published=TRUE WHERE id=$1")
        .bind(page)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let response = router.clone().oneshot(get()).await.unwrap();
    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(response.headers()["location"], "/stranky/legacy");
    let doc:i64=sqlx::query_scalar("INSERT INTO documents(title,status,created_at) VALUES ('Image','published','now') RETURNING id").fetch_one(&app.state.pool).await.unwrap();
    let image:i64=sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES ($1,'picture.png','image/png',8,$2) RETURNING id").bind(doc).bind(b"\x89PNG\r\n\x1a\n".as_slice()).fetch_one(&app.state.pool).await.unwrap();
    let path = format!("/api/v1/legacy-media/{image}");
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,attachment_id,destination) VALUES ('asset','https://vysker.cz/assets/Image.ashx','hash','now','now','{}',$1,$2,$3)").bind(doc).bind(image).bind(format!("/api/v1/attachments/{image}")).execute(&app.state.pool).await.unwrap();
    let response = app.call("GET", &path, None, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    sqlx::query("UPDATE documents SET status='archived' WHERE id=$1")
        .bind(doc)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn renderer_embeds_only_local_imported_media_routes() {
    let rendered = obecni_web::markdown::render(
        "![Safe](/api/v1/legacy-media/123) ![Remote](https://tracker.test/pixel) ![Bad](/api/v1/legacy-media/123?url=remote)",
    );
    assert_eq!(rendered.matches("<img").count(), 1);
    assert!(rendered.contains("src=\"/api/v1/legacy-media/123\""));
}

#[test]
fn missing_event_time_explains_legacy_origin() {
    let event = obecni_web::events::Event {
        starts_at: "2026-09-29T19:00:00+02:00".into(),
        ends_at: "2026-09-29T23:59:59+02:00".into(),
        start_time_known: true,
        end_time_known: false,
        end_date_known: true,
        ..Default::default()
    };
    assert!(event.start_label().contains("19:00"));
    assert_eq!(event.end_label(), "29. 9. 2026 (původní web čas neuváděl)");
    assert!(!event.end_label().contains("23:59"));
}

#[tokio::test]
async fn document_details_identify_imports_without_exposing_hidden_archive_sources() {
    let app = common::App::new().await;
    sqlx::raw_sql("INSERT INTO documents(id,title,description,status,created_at) VALUES
        (1,'Importovaný dokument','Popis','published','now'),
        (2,'Nový dokument','Převedeno z původního webu.','published','now'),
        (3,'Soukromý import','Popis','draft','now');
        INSERT INTO notices(id,title,published_on,status,retain_attachments,review_json) VALUES
        (1,'Importovaný záznam',NULL,'archived',TRUE,
        '{\"archive_title\":\"Importovaný záznam\",\"archive_basis\":\"Local preview\",\"archive_until\":\"9999-12-31\"}'),
        (2,'Nový záznam','2020-01-01','published',FALSE,'{}');
        INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,notice_id,destination) VALUES
        ('doc','https://vysker.cz/assets/File.ashx?id_dokumenty=1','hash','now','now','{}',1,NULL,'/dokumenty/1'),
        ('draft','https://vysker.cz/assets/File.ashx?id_dokumenty=3','hash','now','now','{}',3,NULL,'/dokumenty/3'),
        ('notice','https://vysker.cz/assets/File.ashx?id_dokumenty=2','hash','now','now','{}',NULL,1,'/uredni-deska/1')")
        .execute(&app.state.pool).await.unwrap();
    let imported = obecni_web::catalog::document_from_pool(&app.state.pool, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        imported.import_origin.unwrap().source_url.as_deref(),
        Some("https://vysker.cz/assets/File.ashx?id_dokumenty=1")
    );
    let native = obecni_web::catalog::document_from_pool(&app.state.pool, 2)
        .await
        .unwrap()
        .unwrap();
    assert!(native.import_origin.is_none());
    assert!(
        obecni_web::catalog::document_from_pool(&app.state.pool, 3)
            .await
            .unwrap()
            .is_none()
    );
    let imported = obecni_web::content::notice_from_pool(&app.state.pool, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(imported.archived);
    assert!(imported.description.is_empty());
    assert_eq!(
        imported.import_origin.unwrap().source_url.as_deref(),
        Some("https://vysker.cz/assets/File.ashx?id_dokumenty=2")
    );
    let native = obecni_web::content::notice_from_pool(&app.state.pool, 2)
        .await
        .unwrap()
        .unwrap();
    assert!(native.import_origin.is_none());
    sqlx::query("UPDATE notices SET review_json='{\"archive_until\":\"2020-01-01\"}' WHERE id=1")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let expired = obecni_web::content::notice_from_pool(&app.state.pool, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(!expired.retain_files);
    assert!(expired.import_origin.unwrap().source_url.is_none());
}

#[tokio::test]
async fn local_notice_preview_preserves_attachments_but_cannot_launch_in_production() {
    let app = common::App::new().await;
    sqlx::raw_sql("INSERT INTO documents(id,title,status,created_at) VALUES (1,'Source','published','now');
        INSERT INTO legacy_sources(source_key,source_url,fingerprint,captured_at,imported_at,metadata,document_id,destination)
        VALUES ('test-file','https://vysker.cz/assets/File.ashx','hash','now','now','{}',1,'/dokumenty/1');
        INSERT INTO notices(id,title,published_on,withdraw_on,status,retain_attachments,review_json)
        VALUES (1,'Original title','2020-01-01','2020-02-01','archived',TRUE,
        '{\"archive_title\":\"Original title\",\"archive_basis\":\"Local preview\",\"archive_until\":\"9999-12-31\"}');
        INSERT INTO attachments(notice_id,name,content_type,size_bytes,data) VALUES (1,'file.pdf','application/pdf',4,'data');
        INSERT INTO legacy_notice_imports(source_key,notice_id,imported_at,preview_published,metadata)
        VALUES ('test-file',1,'now',TRUE,'{}')")
        .execute(&app.state.pool).await.unwrap();
    obecni_web::backend::notices::maintenance(&app.state, time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    let files = obecni_web::backend::documents::public_notice_files(
        &app.state.pool,
        1,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert!(files[0].available);
    let imported = obecni_web::content::notice_from_pool(&app.state.pool, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        imported.import_origin.unwrap().source_url.as_deref(),
        Some("https://vysker.cz/assets/File.ashx")
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/attachments/{}", files[0].id),
            None,
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    let mut config = (*app.state.config).clone();
    config.production = true;
    let prod = obecni_web::backend::Backend::new(app.state.pool.clone(), config);
    let error = obecni_web::backend::server::check_launch_content(&prod)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("místní náhled"));
}
