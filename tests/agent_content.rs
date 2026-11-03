#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::App;
use obecni_web::backend::{agent_content::call, attachment_index::EXTRACTOR_VERSION};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

async fn document(app: &App, title: &str, description: &str, public: bool) -> i64 {
    // Published inserts become public through the lifecycle guard. A private
    // fixture must use draft status instead of contradicting that transition.
    let (id, actual_public): (i64, bool) = sqlx::query_as("INSERT INTO documents(title,description,status,public_access,created_at) VALUES ($1,$2,CASE WHEN $3::boolean THEN 'published' ELSE 'draft' END,$3,'2026-11-01') RETURNING id,public_access")
        .bind(title).bind(description).bind(public).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(actual_public, public);
    id
}

async fn attachment(app: &App, document: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES ($1,$2,'application/pdf',8,$3) RETURNING id")
        .bind(document).bind(name).bind(b"%PDF-1.4".to_vec()).fetch_one(&app.state.pool).await.unwrap()
}
async fn index(app: &App, attachment: i64, status: &str, pages: &[(i32, &str, &str)]) {
    sqlx::query("DELETE FROM attachment_extractions WHERE attachment_id=$1")
        .bind(attachment)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method,warnings,error) SELECT id,encode(sha256(coalesce(data,''::bytea)),'hex'),name,$2,$3,'ocr','[]','Private worker diagnostic' FROM attachments WHERE id=$1")
        .bind(attachment).bind(EXTRACTOR_VERSION).bind(status).execute(&app.state.pool).await.unwrap();
    for (page, label, content) in pages {
        sqlx::query("INSERT INTO attachment_search_pages(attachment_id,page,label,content) VALUES ($1,$2,$3,$4)")
            .bind(attachment).bind(page).bind(label).bind(content).execute(&app.state.pool).await.unwrap();
    }
}
async fn read(app: &App, name: &str, args: Value) -> Value {
    call(&app.state, name, args).await.unwrap()
}
fn assert_source(value: &Value) {
    assert!(
        value["source_url"]
            .as_str()
            .unwrap()
            .starts_with("http://localhost:3000/")
    );
    assert_eq!(value["language"], "cs");
    assert_eq!(
        OffsetDateTime::parse(value["fetched_at"].as_str().unwrap(), &Rfc3339)
            .unwrap()
            .offset()
            .whole_seconds(),
        0
    );
}

#[tokio::test]
async fn default_read_returns_the_complete_multilingual_transcription_with_citable_locations() {
    let app = App::new().await;
    let did = document(&app, "Zápis", "Jednání zastupitelstva", true).await;
    let aid = attachment(&app, did, "zápis.pdf").await;
    let first = "Žluťoučký kůň: hlasování o pronájmu. <script>source data</script>";
    index(
        &app,
        aid,
        "ready",
        &[
            (1, "Strana 1", first),
            (2, "Strana 2", "Druhý návrh byl přijat."),
        ],
    )
    .await;
    let value = read(&app, "read_attachment_text", json!({"attachment_id":aid})).await;
    assert_source(&value);
    assert_eq!(value["document_id"], did);
    assert_eq!(value["complete"], true);
    assert_eq!(value["has_more"], false);
    assert!(value["next_cursor"].is_null());
    assert_eq!(value["location_kind"], "pdf_page");
    assert_eq!(value["parts_total"], 2);
    assert_eq!(value["parts"][0]["text"], first);
    assert_eq!(value["parts"][1]["page"], 2);
    assert_eq!(value["parts"][1]["label"], "Strana 2");
    assert!(
        value["text_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/attachments/{aid}/text"))
    );
    assert!(value["ocr_warning"].as_str().unwrap().contains("errors"));
    assert!(!value.to_string().contains("Private worker diagnostic"));
    let office = attachment(&app, did, "tabulka.ods").await;
    index(
        &app,
        office,
        "ready",
        &[(1, "List: Rozpočet", "Příjmy: 10 000 Kč")],
    )
    .await;
    let office = read(
        &app,
        "read_attachment_text",
        json!({"attachment_id":office}),
    )
    .await;
    assert_eq!(office["location_kind"], "document_part");
    assert_eq!(office["parts"][0]["label"], "List: Rozpočet");
}

#[tokio::test]
async fn serialized_budget_and_unicode_cursors_preserve_every_character_and_reject_stale_revisions()
{
    let app = App::new().await;
    let did = document(&app, "Dlouhý zápis", "", true).await;
    let aid = attachment(&app, did, "dlouhy.pdf").await;
    let first = format!("{}{}", "\"\\".repeat(100_000), "Ž💡\n".repeat(10_000));
    let second = "Konec druhé strany.";
    index(
        &app,
        aid,
        "ready",
        &[(1, "Strana 1", &first), (2, "Strana 2", second)],
    )
    .await;
    let mut next = json!({"attachment_id":aid});
    let mut restored: BTreeMap<i64, String> = BTreeMap::new();
    let mut chunks = 0;
    loop {
        let value = read(&app, "read_attachment_text", next).await;
        chunks += 1;
        assert!(serde_json::to_vec(&value).unwrap().len() <= 96 * 1024);
        assert!(
            serde_json::to_vec(&rmcp::model::CallToolResult::structured(value.clone()))
                .unwrap()
                .len()
                <= 256 * 1024
        );
        assert_eq!(value["complete"], false);
        for part in value["parts"].as_array().unwrap() {
            let combined = restored.entry(part["page"].as_i64().unwrap()).or_default();
            assert_eq!(
                part["offset"].as_u64().unwrap() as usize,
                combined.chars().count()
            );
            combined.push_str(part["text"].as_str().unwrap());
        }
        let Some(cursor) = value["next_cursor"].as_str() else {
            break;
        };
        next = json!({"attachment_id":aid,"cursor":cursor});
        assert!(chunks < 20);
    }
    assert!(chunks > 1);
    assert_eq!(restored[&1], first);
    assert_eq!(restored[&2], second);
    let short = read(
        &app,
        "read_attachment_text",
        json!({"attachment_id":aid,"max_chars":7}),
    )
    .await;
    assert_eq!(
        short["parts"][0]["text"].as_str().unwrap().chars().count(),
        7
    );
    let cursor = short["next_cursor"].as_str().unwrap();
    let other = attachment(&app, did, "jiny.pdf").await;
    index(&app, other, "ready", &[(1, "Strana 1", "Jiný text")]).await;
    assert_eq!(
        call(
            &app.state,
            "read_attachment_text",
            json!({"attachment_id":other,"cursor":cursor})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE attachment_extractions SET started_at=started_at+interval '1 second' WHERE attachment_id=$1")
        .bind(aid).execute(&app.state.pool).await.unwrap();
    assert_eq!(
        call(
            &app.state,
            "read_attachment_text",
            json!({"attachment_id":aid,"cursor":cursor})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE attachments SET name='prejmenovany.pdf' WHERE id=$1")
        .bind(aid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        read(&app, "read_attachment_text", json!({"attachment_id":aid})).await["status"],
        "pending"
    );
}

#[tokio::test]
async fn coverage_counts_only_current_public_files_and_permissions_are_rechecked_for_continuations()
{
    let app = App::new().await;
    let did = document(&app, "Veřejné", "", true).await;
    let ready = attachment(&app, did, "hotovo.pdf").await;
    index(&app, ready, "ready", &[(1, "Strana 1", "Veřejný text")]).await;
    let pending = attachment(&app, did, "ceka.docx").await;
    let failed = attachment(&app, did, "chyba.pdf").await;
    index(&app, failed, "failed", &[]).await;
    let processing = attachment(&app, did, "zpracovani.odt").await;
    index(&app, processing, "processing", &[]).await;
    attachment(&app, did, "obrazek.png").await;
    let private_doc = document(&app, "Secret draft", "Secret metadata", false).await;
    let hidden = attachment(&app, private_doc, "secret.pdf").await;
    index(&app, hidden, "ready", &[(1, "Strana 1", "Secret text")]).await;
    let hidden_file = attachment(&app, did, "hidden.pdf").await;
    sqlx::query("UPDATE attachments SET is_public=FALSE WHERE id=$1")
        .bind(hidden_file)
        .execute(&app.state.pool)
        .await
        .unwrap();
    index(
        &app,
        hidden_file,
        "ready",
        &[(1, "Strana 1", "Secret text")],
    )
    .await;
    let value = read(&app, "get_content_coverage", json!({})).await;
    assert_source(&value);
    assert_eq!(value["unit"], "currently_public_attachments");
    assert_eq!(value["total"], 5);
    for status in ["ready", "pending", "processing", "failed", "unsupported"] {
        assert_eq!(value[status], 1);
    }
    assert_eq!(value["transcription_coverage_complete"], false);
    for aid in [hidden, hidden_file] {
        assert_eq!(
            call(
                &app.state,
                "read_attachment_text",
                json!({"attachment_id":aid})
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        read(
            &app,
            "read_attachment_text",
            json!({"attachment_id":pending})
        )
        .await["status"],
        "pending"
    );
    let failure = read(
        &app,
        "read_attachment_text",
        json!({"attachment_id":failed}),
    )
    .await;
    assert_eq!(failure["status"], "failed");
    assert!(!failure.to_string().contains("Private worker diagnostic"));
    let start = read(
        &app,
        "read_attachment_text",
        json!({"attachment_id":ready,"max_chars":1}),
    )
    .await;
    sqlx::query("UPDATE documents SET attachments_public=FALSE WHERE id=$1")
        .bind(did)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app.state,
            "read_attachment_text",
            json!({"attachment_id":ready,"cursor":start["next_cursor"]})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        read(&app, "get_content_coverage", json!({})).await["total"],
        0
    );
}

#[tokio::test]
async fn search_reuses_title_ranking_and_counts_documents_instead_of_occurrences() {
    let app = App::new().await;
    let title = document(&app, "Pronájem hospody", "", true).await;
    let body = document(&app, "Zápis z jednání", "", true).await;
    let aid = attachment(&app, body, "zapis.pdf").await;
    index(
        &app,
        aid,
        "ready",
        &[
            (1, "Strana 1", "Pronájem hospody. Pronájem hospody."),
            (2, "Strana 2", "Pronájem hospody."),
        ],
    )
    .await;
    let hidden = document(&app, "Pronájem hospody soukromě", "", false).await;
    let hidden_file = attachment(&app, hidden, "zapis.pdf").await;
    index(
        &app,
        hidden_file,
        "ready",
        &[(1, "Strana 1", "Pronájem hospody.")],
    )
    .await;
    let value = read(&app, "search", json!({"query":"pronajem hospody"})).await;
    assert_source(&value);
    assert_eq!(value["total"], 2);
    assert_eq!(value["total_unit"], "documents_and_pages");
    assert_eq!(value["items"][0]["id"], title);
    assert_eq!(value["items"][1]["id"], body);
    assert_eq!(value["items"][1]["attachment"]["id"], aid);
    assert!(
        value["items"][1]["attachment"]["source_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/attachments/{aid}"))
    );
    assert!(value["items"][1].get("preview_html").is_none());
    let listing = read(&app, "list_documents", json!({"limit":1})).await;
    assert_eq!(listing["items"].as_array().unwrap().len(), 1);
    assert_eq!(listing["next_offset"], 1);
    let next = read(&app, "list_documents", json!({"limit":1,"offset":1})).await;
    assert_ne!(listing["items"][0]["id"], next["items"][0]["id"]);
    assert!(next["next_offset"].is_null());
    for query in ["x".repeat(201), "a b c d e f g h i j k".to_string()] {
        assert_eq!(
            call(&app.state, "search", json!({"query":query}))
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn document_redaction_and_page_version_checks_match_the_public_site() {
    let app = App::new().await;
    let did = document(&app, "Secret personal title", "Secret personal text", true).await;
    sqlx::query("UPDATE documents SET metadata_restricted=TRUE,public_title='Veřejný záznam',reference_number='Secret reference',issuer='Secret issuer',attachments_public=FALSE WHERE id=$1")
        .bind(did).execute(&app.state.pool).await.unwrap();
    let record = read(&app, "get_document", json!({"id":did})).await;
    assert_eq!(record["title"], "Veřejný záznam");
    assert!(!record.to_string().contains("Secret"));
    let full = "# Obec\nŽluťoučký kůň.";
    sqlx::query("INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('test-public','Obec',$1,TRUE,'2026-11-01'),('test-private','Soukromé','Tajné',FALSE,'2026-11-01')")
        .bind(full).execute(&app.state.pool).await.unwrap();
    let first = read(
        &app,
        "get_page",
        json!({"slug":"test-public","max_chars":8}),
    )
    .await;
    let next = read(&app,"get_page",json!({"slug":"test-public","offset":first["next_offset"],"expected_version":first["version"]})).await;
    assert_eq!(
        format!(
            "{}{}",
            first["content"].as_str().unwrap(),
            next["content"].as_str().unwrap()
        ),
        full
    );
    assert_eq!(first["format"], "markdown");
    sqlx::query("UPDATE pages SET version=version+1 WHERE slug='test-public'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app.state,
            "get_page",
            json!({"slug":"test-public","expected_version":first["version"]})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app.state, "get_page", json!({"slug":"test-private"}))
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app.state, "get_document", json!({"id":did,"publish":true}))
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn event_windows_include_each_public_occurrence_and_preserve_prague_daylight_time() {
    let app = App::new().await;
    let recurring: i64 = sqlx::query_scalar("INSERT INTO events(title,starts_at,ends_at,published,updated_at,repeat_every_14_days,repeat_until) VALUES ('Opakované setkání','2026-03-22T12:00:00+01:00','2026-03-22T13:00:00+01:00',TRUE,'2026-11-01',TRUE,'2026-04-19') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO events(title,starts_at,ends_at,published,cancelled,updated_at) VALUES ('Zrušená akce','2026-04-04T12:00:00Z','2026-04-04T13:00:00Z',TRUE,TRUE,'2026-11-01'),('Soukromá akce','2026-04-03T12:00:00Z','2026-04-03T13:00:00Z',FALSE,FALSE,'2026-11-01')")
        .execute(&app.state.pool).await.unwrap();
    let value = read(
        &app,
        "list_events",
        json!({"from":"2026-03-22","to":"2026-04-19"}),
    )
    .await;
    assert_eq!(value["items"].as_array().unwrap().len(), 4);
    assert_eq!(value["unit"], "event_occurrences");
    assert_eq!(value["items"][0]["id"], recurring);
    assert!(
        value["items"][0]["occurrence_starts_at"]
            .as_str()
            .unwrap()
            .contains("12:00:00+01:00")
    );
    assert_eq!(value["items"][1]["cancelled"], true);
    assert!(
        value["items"][2]["occurrence_starts_at"]
            .as_str()
            .unwrap()
            .contains("12:00:00+02:00")
    );
    assert!(!value.to_string().contains("Soukromá akce"));
    let second = read(
        &app,
        "list_events",
        json!({"from":"2026-03-22","to":"2026-04-19","limit":1,"offset":1}),
    )
    .await;
    assert_eq!(second["items"][0]["title"], "Zrušená akce");
    assert_eq!(second["next_offset"], 2);
    assert_eq!(
        call(
            &app.state,
            "list_events",
            json!({"from":"2026-01-01","to":"2027-01-02"})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn rental_windows_filter_and_paginate_the_same_public_occupancy_as_the_site() {
    let app = App::new().await;
    let aid: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ('Veřejný sál',100,100,'private@example.test') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    let from = obecni_web::backend::today(OffsetDateTime::now_utc()) + time::Duration::days(7);
    for days in [0, 1, 2, 40] {
        let day = from + time::Duration::days(days);
        sqlx::query("INSERT INTO rental_blocks(asset_id,start_date,end_date) VALUES ($1,$2,$2)")
            .bind(aid)
            .bind(day)
            .execute(&app.state.pool)
            .await
            .unwrap();
    }
    let to = from + time::Duration::days(2);
    let first = read(
        &app,
        "get_rental_availability",
        json!({"asset_id":aid,"from":from.to_string(),"to":to.to_string(),"limit":2}),
    )
    .await;
    assert_source(&first);
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["next_offset"], 2);
    assert_eq!(first["items"][0]["start_date"], from.to_string());
    assert_eq!(first["items"][0]["status"], "booked");
    assert!(!first.to_string().contains("private@example.test"));
    let second = read(
        &app,
        "get_rental_availability",
        json!({"asset_id":aid,"from":from.to_string(),"to":to.to_string(),"limit":2,"offset":2}),
    )
    .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["start_date"], to.to_string());
    assert!(second["next_offset"].is_null());
    let yesterday = obecni_web::backend::today(OffsetDateTime::now_utc()) - time::Duration::days(1);
    assert_eq!(
        call(
            &app.state,
            "get_rental_availability",
            json!({"asset_id":aid,"from":yesterday.to_string()})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn transcription_pages_preserve_original_numbers_and_explicit_selection() {
    let app = App::new().await;
    let did = document(&app, "Vybrané strany", "", true).await;
    let aid = attachment(&app, did, "vyber.pdf").await;
    index(
        &app,
        aid,
        "ready",
        &[
            (2, "Strana 2", "Druhá strana"),
            (4, "Strana 4", "Čtvrtá strana"),
        ],
    )
    .await;
    let entire = read(&app, "read_attachment_text", json!({"attachment_id":aid})).await;
    assert_eq!(entire["parts_total"], 2);
    assert_eq!(entire["complete"], true);
    assert_eq!(entire["parts"][0]["page"], 2);
    assert_eq!(entire["parts"][1]["page"], 4);
    let selected = read(
        &app,
        "read_attachment_text",
        json!({"attachment_id":aid,"page":4}),
    )
    .await;
    assert_eq!(selected["parts"].as_array().unwrap().len(), 1);
    assert_eq!(selected["parts"][0]["text"], "Čtvrtá strana");
    assert_eq!(selected["complete"], false);
    assert_eq!(selected["has_more"], false);
    assert!(selected["next_cursor"].is_null());
    assert_eq!(
        call(
            &app.state,
            "read_attachment_text",
            json!({"attachment_id":aid,"page":3})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    let first = read(
        &app,
        "read_attachment_text",
        json!({"attachment_id":aid,"max_chars":1}),
    )
    .await;
    assert_eq!(
        call(
            &app.state,
            "read_attachment_text",
            json!({"attachment_id":aid,"page":2,"cursor":first["next_cursor"]})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn expiration_and_removed_originals_hide_even_a_retained_transcription_index() {
    let app = App::new().await;
    for condition in [
        "expired_document",
        "expired_posting_files",
        "removed_original",
    ] {
        let did = document(
            &app,
            "Secret expired title",
            "Secret expired metadata",
            true,
        )
        .await;
        let aid = attachment(&app, did, "secret.pdf").await;
        index(
            &app,
            aid,
            "ready",
            &[(1, "Strana 1", "Secret retained text")],
        )
        .await;
        let first = read(
            &app,
            "read_attachment_text",
            json!({"attachment_id":aid,"max_chars":1}),
        )
        .await;
        match condition {
            "expired_document" => {
                sqlx::query("UPDATE documents SET public_until='2000-01-01' WHERE id=$1")
                    .bind(did)
                    .execute(&app.state.pool)
                    .await
                    .unwrap();
            }
            "expired_posting_files" => {
                sqlx::query("UPDATE documents SET metadata_restricted=TRUE,public_title='Veřejný záznam',posting_public_until='2000-01-01' WHERE id=$1").bind(did).execute(&app.state.pool).await.unwrap();
            }
            _ => {
                sqlx::query("UPDATE attachments SET removed_at='2026-11-01T12:00:00Z',data=NULL WHERE id=$1").bind(aid).execute(&app.state.pool).await.unwrap();
                // Simulate a stale index retained during a restore. The current
                // original and parent permissions must still govern every read.
                index(
                    &app,
                    aid,
                    "ready",
                    &[(1, "Strana 1", "Secret retained text")],
                )
                .await;
            }
        }
        for args in [
            json!({"attachment_id":aid}),
            json!({"attachment_id":aid,"cursor":first["next_cursor"]}),
        ] {
            assert_eq!(
                call(&app.state, "read_attachment_text", args)
                    .await
                    .unwrap_err()
                    .0,
                StatusCode::NOT_FOUND,
                "{condition}"
            );
        }
        assert_eq!(
            read(&app, "get_content_coverage", json!({})).await["total"],
            0,
            "{condition}"
        );
        if condition == "expired_document" {
            assert_eq!(
                call(&app.state, "get_document", json!({"id":did}))
                    .await
                    .unwrap_err()
                    .0,
                StatusCode::NOT_FOUND
            );
        } else {
            let detail = read(&app, "get_document", json!({"id":did})).await;
            assert!(detail["attachments"].as_array().unwrap().is_empty());
            if condition == "expired_posting_files" {
                assert!(!detail.to_string().contains("Secret"));
            }
        }
    }
    assert_eq!(
        call(
            &app.state,
            "get_page",
            json!({"slug":"any-page","offset":100001})
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
}
