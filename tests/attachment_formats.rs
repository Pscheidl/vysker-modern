#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use http_body_util::BodyExt;
use obecni_web::backend::{
    attachment_index::{RunOutcome, WorkerOptions, run_once},
    search,
};
use obecni_web::search::SearchQuery;
use serde_json::json;

#[tokio::test]
async fn every_supported_format_can_be_uploaded_indexed_found_and_downloaded() {
    let app = App::new().await;
    let options = WorkerOptions::default();
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search");
    for filename in [
        "text.pdf",
        "scan.pdf",
        "mixed.pdf",
        "hybrid.pdf",
        "obec.docx",
        "obec.odt",
        "obec.xlsx",
        "obec.ods",
        "obec.csv",
        "obec.txt",
        "obec.json",
        "obec.xml",
    ] {
        let created = app.call("POST", "/api/v1/admin/documents", Some(json!({"title":format!("Zkušební příloha {filename}"),"description":"Ověření vyhledávání"})), true).await;
        assert_eq!(created.status(), StatusCode::CREATED, "{filename}");
        let id = body_json(created).await["id"].as_i64().unwrap();
        let bytes = std::fs::read(fixtures.join(filename)).unwrap();
        let uploaded = app
            .upload(
                &format!("/api/v1/admin/documents/{id}/attachments"),
                filename,
                &bytes,
            )
            .await;
        assert_eq!(uploaded.status(), StatusCode::CREATED, "{filename}");
        let attachment = body_json(uploaded).await["id"].as_i64().unwrap();
        let indexed = run_once(&app.state.pool, &options).await.unwrap();
        assert!(
            matches!(indexed, RunOutcome::Indexed { attachment_id, .. } if attachment_id == attachment),
            "{filename}: {indexed:?}"
        );
        let query = SearchQuery {
            q: "hospod 15000".into(),
            kind: "document".into(),
            ..Default::default()
        };
        assert!(
            !search::query(&app.state.pool, &query)
                .await
                .unwrap()
                .items
                .iter()
                .any(|hit| hit.id == id),
            "Draft {filename} leaked"
        );
        let published = app
            .call(
                "POST",
                &format!("/api/v1/admin/documents/{id}/publish"),
                Some(json!({})),
                true,
            )
            .await;
        assert_eq!(published.status(), StatusCode::NO_CONTENT, "{filename}");
        let found = search::query(&app.state.pool, &query).await.unwrap();
        assert!(
            found.items.iter().any(|hit| hit.id == id),
            "{filename} absent after indexing"
        );
        let text_url = format!("/api/v1/attachments/{attachment}/text");
        let detail = body_json(
            app.call("GET", &format!("/api/v1/documents/{id}"), None, false)
                .await,
        )
        .await;
        assert_eq!(detail["attachments"][0]["text_url"], text_url, "{filename}");
        for url in [&text_url, &format!("{text_url}/download")] {
            let response = app.call("GET", url, None, false).await;
            assert_eq!(response.status(), StatusCode::OK, "{filename}");
            assert_eq!(
                response.headers()["content-type"],
                "text/plain; charset=utf-8"
            );
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(
                text.to_lowercase().contains("hospod") && text.contains("15000"),
                "{filename}"
            );
        }
        assert_eq!(
            app.call(
                "GET",
                &format!("/api/v1/attachments/{attachment}"),
                None,
                false
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn malformed_json_and_xml_entities_are_rejected_at_upload() {
    let app = App::new().await;
    let created = body_json(
        app.call(
            "POST",
            "/api/v1/admin/documents",
            Some(json!({"title":"Datové přílohy"})),
            true,
        )
        .await,
    )
    .await;
    let id = created["id"].as_i64().unwrap();
    for (name, data) in [
        ("broken.json", b"{not json}".as_slice()),
        ("broken.xml", b"not xml".as_slice()),
        (
            "entity.xml",
            b"<!DOCTYPE x [<!ENTITY a SYSTEM 'file:///etc/passwd'>]><x>&a;</x>".as_slice(),
        ),
    ] {
        assert_eq!(
            app.upload(
                &format!("/api/v1/admin/documents/{id}/attachments"),
                name,
                data
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "{name}"
        );
    }
}
