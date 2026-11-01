#![cfg(feature = "ssr")]
mod common;

use axum::{http::StatusCode, response::Response};
use common::App;
use http_body_util::BodyExt;
use obecni_web::backend::attachment_index::EXTRACTOR_VERSION;
use serde_json::json;

async fn body(response: Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

async fn index(app: &App, attachment: i64, method: &str, pages: &[(i32, &str, &str)]) {
    sqlx::query("DELETE FROM attachment_extractions WHERE attachment_id=$1")
        .bind(attachment)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method,warnings) SELECT id,encode(sha256(coalesce(data,''::bytea)),'hex'),name,$2,'ready',$3,$4 FROM attachments WHERE id=$1")
        .bind(attachment).bind(EXTRACTOR_VERSION).bind(method).bind(sqlx::types::Json(json!([])))
        .execute(&app.state.pool).await.unwrap();
    for (page, label, content) in pages {
        sqlx::query("INSERT INTO attachment_search_pages(attachment_id,page,label,content) VALUES ($1,$2,$3,$4)")
            .bind(attachment).bind(page).bind(label).bind(content)
            .execute(&app.state.pool).await.unwrap();
    }
}

async fn fixture(app: &App, name: &str, method: &str, pages: &[(i32, &str, &str)]) -> (i64, i64) {
    let document: i64 = sqlx::query_scalar("INSERT INTO documents(title,status,public_access,created_at) VALUES ('Veřejný zápis','published',TRUE,'2026-11-01') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    let attachment: i64 = sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES ($1,$2,'application/pdf',8,$3) RETURNING id")
        .bind(document).bind(name).bind(b"%PDF-1.4".to_vec())
        .fetch_one(&app.state.pool).await.unwrap();
    index(app, attachment, method, pages).await;
    (document, attachment)
}

fn paths(attachment: i64) -> [String; 2] {
    [
        format!("/api/v1/attachments/{attachment}/text"),
        format!("/api/v1/attachments/{attachment}/text/download"),
    ]
}

#[tokio::test]
async fn preview_and_download_serve_every_page_as_literal_utf8_without_javascript() {
    let app = App::new().await;
    let full_page = format!(
        "{} <script>alert('doslovný text')</script> Konec první strany.",
        "Žluťoučký kůň. ".repeat(500)
    );
    let name = "Zápis z jednání \"Žluťoučký\".pdf";
    let (document, attachment) = fixture(
        &app,
        name,
        "mixed",
        &[
            (2, "Strana 2", "Druhá strana: pronájem hospody."),
            (1, "Strana 1", &full_page),
        ],
    )
    .await;
    sqlx::query("UPDATE attachment_extractions SET warnings=$2 WHERE attachment_id=$1")
        .bind(attachment)
        .bind(sqlx::types::Json(json!(["Část obsahu přečetlo OCR."])))
        .execute(&app.state.pool)
        .await
        .unwrap();
    let [preview_path, download_path] = paths(attachment);
    let preview = app.call("GET", &preview_path, None, false).await;
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(
        preview.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(preview.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        preview.headers()["content-security-policy"],
        "sandbox; default-src 'none'"
    );
    let cache = preview.headers()["cache-control"].to_str().unwrap();
    assert!(cache.contains("no-store") && cache.contains("private"));
    let disposition = preview.headers()["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("inline;"));
    let encoded = disposition.split("filename*=UTF-8''").nth(1).unwrap();
    assert_eq!(
        percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .unwrap(),
        format!("{name}.txt")
    );
    let text = body(preview).await;
    assert!(text.contains(&format!("Soubor: {name}")));
    assert!(text.contains(&format!(
        "Dokument: http://localhost:3000/dokumenty/{document}"
    )));
    assert!(text.contains(&format!(
        "Originál: http://localhost:3000/api/v1/attachments/{attachment}"
    )));
    assert!(text.contains("Automatický přepis OCR může obsahovat chyby"));
    assert!(text.contains("Část obsahu přečetlo OCR."));
    assert!(text.contains(&full_page));
    assert!(text.contains("Druhá strana: pronájem hospody."));
    assert!(text.find("=== Strana 1 ===").unwrap() < text.find("=== Strana 2 ===").unwrap());
    assert!(!text.contains("&lt;script&gt;"));

    let download = app.call("GET", &download_path, None, false).await;
    assert_eq!(download.status(), StatusCode::OK);
    assert!(
        download.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(body(download).await, text);
    let head = app.call("HEAD", &preview_path, None, false).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["content-type"], "text/plain; charset=utf-8");
    assert!(body(head).await.is_empty());
    let original = app
        .call(
            "GET",
            &format!("/api/v1/attachments/{attachment}"),
            None,
            false,
        )
        .await;
    assert_eq!(original.status(), StatusCode::OK);
    assert_eq!(body(original).await, "%PDF-1.4");
}

#[tokio::test]
async fn current_original_permissions_apply_even_to_retained_index_and_signed_in_admin() {
    let app = App::new().await;
    let conditions = [
        ("documents", "public_access=FALSE"),
        ("documents", "access_blocked=TRUE,public_access=FALSE"),
        ("documents", "public_until='2000-01-01'"),
        ("documents", "attachments_public=FALSE"),
        (
            "documents",
            "board_status='archived',posting_public_until='2000-01-01'",
        ),
        (
            "documents",
            "metadata_restricted=TRUE,posting_public_until='2000-01-01'",
        ),
        ("attachments", "is_public=FALSE"),
        ("attachments", "removed_at='2026-11-01T12:00:00Z',data=NULL"),
        ("attachments", "data=NULL"),
    ];
    for (table, condition) in conditions {
        let (document, attachment) = fixture(
            &app,
            "zápis.pdf",
            "ocr",
            &[(1, "Strana 1", "Neveřejné údaje.")],
        )
        .await;
        let statement = format!("UPDATE {table} SET {condition} WHERE id=$1");
        sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
            .bind(if table == "documents" {
                document
            } else {
                attachment
            })
            .execute(&app.state.pool)
            .await
            .unwrap_or_else(|error| panic!("Invalid visibility fixture {condition}: {error}"));
        // Re-create even an index accidentally retained by an operator. Current
        // permissions, not the state when text was generated, decide access.
        index(
            &app,
            attachment,
            "ocr",
            &[(1, "Strana 1", "Neveřejné údaje.")],
        )
        .await;
        for path in paths(attachment) {
            for authenticated in [false, true] {
                let response = app.call("GET", &path, None, authenticated).await;
                assert_eq!(
                    response.status(),
                    StatusCode::NOT_FOUND,
                    "{condition}, admin={authenticated}"
                );
                let cache = response.headers()["cache-control"].to_str().unwrap();
                assert!(cache.contains("private") && cache.contains("no-store"));
                assert!(!body(response).await.contains("Neveřejné údaje"));
            }
        }
        let original = app
            .call(
                "GET",
                &format!("/api/v1/attachments/{attachment}"),
                None,
                false,
            )
            .await;
        assert_eq!(original.status(), StatusCode::NOT_FOUND, "{condition}");
    }
}

#[tokio::test]
async fn unavailable_or_stale_text_is_not_public_even_while_original_is_available() {
    let app = App::new().await;
    for state in [
        "missing",
        "processing",
        "failed",
        "old_name",
        "old_version",
        "empty",
    ] {
        let (_, attachment) = fixture(
            &app,
            "zápis.pdf",
            "text",
            &[(1, "Strana 1", "Starý přepis.")],
        )
        .await;
        let sql = match state {
            "missing" => "DELETE FROM attachment_extractions WHERE attachment_id=$1",
            "processing" => {
                "UPDATE attachment_extractions SET status='processing' WHERE attachment_id=$1"
            }
            "failed" => "UPDATE attachment_extractions SET status='failed' WHERE attachment_id=$1",
            "old_name" => {
                "UPDATE attachment_extractions SET source_name='původní.pdf' WHERE attachment_id=$1"
            }
            "old_version" => {
                "UPDATE attachment_extractions SET extractor_version=0 WHERE attachment_id=$1"
            }
            "empty" => "DELETE FROM attachment_search_pages WHERE attachment_id=$1",
            _ => unreachable!(),
        };
        sqlx::query(sql)
            .bind(attachment)
            .execute(&app.state.pool)
            .await
            .unwrap();
        for path in paths(attachment) {
            assert_eq!(
                app.call("GET", &path, None, false).await.status(),
                StatusCode::NOT_FOUND,
                "{state}"
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
    for path in paths(i64::MAX) {
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn replacing_or_renaming_original_invalidates_text_before_it_can_be_served() {
    let app = App::new().await;
    let (_, attachment) = fixture(
        &app,
        "starý.pdf",
        "text",
        &[(1, "Strana 1", "Původní soukromý text.")],
    )
    .await;
    sqlx::query("UPDATE attachments SET data=$2 WHERE id=$1")
        .bind(attachment)
        .bind(b"%PDF-new".to_vec())
        .execute(&app.state.pool)
        .await
        .unwrap();
    for path in paths(attachment) {
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    index(
        &app,
        attachment,
        "text",
        &[(1, "Strana 1", "Aktuální přepis.")],
    )
    .await;
    let text = body(app.call("GET", &paths(attachment)[0], None, false).await).await;
    assert!(text.contains("Aktuální přepis."));
    assert!(!text.contains("Původní soukromý text."));
    sqlx::query("UPDATE attachments SET name='nový.pdf' WHERE id=$1")
        .bind(attachment)
        .execute(&app.state.pool)
        .await
        .unwrap();
    for path in paths(attachment) {
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn office_section_labels_and_unusual_legacy_filename_stay_plain_and_header_safe() {
    let app = App::new().await;
    let (_, attachment) = fixture(
        &app,
        "Žluťoučký\r\n../\".docx",
        "text",
        &[
            (1, "Dokument", "Příliš žluťoučký kůň."),
            (2, "Poznámky a záhlaví", "Poznámka pod čarou."),
        ],
    )
    .await;
    let response = app.call("GET", &paths(attachment)[1], None, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response.headers()["content-disposition"].to_str().unwrap();
    assert!(disposition.is_ascii());
    let encoded = disposition.split("filename*=UTF-8''").nth(1).unwrap();
    let decoded = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .unwrap();
    assert_eq!(decoded, "Žluťoučký.._\".docx.txt");
    assert!(!decoded.chars().any(char::is_control));
    let text = body(response).await;
    assert!(text.contains("=== Dokument ==="));
    assert!(text.contains("=== Poznámky a záhlaví ==="));
    assert!(!text.contains("Strana 1"));
    assert!(!text.contains("OCR"));
}
