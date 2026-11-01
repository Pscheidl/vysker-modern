#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{App, body_json};
use obecni_web::backend::{attachment_index::EXTRACTOR_VERSION, auth};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const STATUS: &str = "/api/v1/admin/attachment-index/status";

async fn attachment(pool: &PgPool, name: &str) -> (i64, i64) {
    let document: i64 = sqlx::query_scalar("INSERT INTO documents(title,created_at) VALUES('Koncept dokumentu','2026-11-01') RETURNING id")
        .fetch_one(pool).await.unwrap();
    let attachment = sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES($1,$2,'application/octet-stream',4,'test'::bytea) RETURNING id")
        .bind(document).bind(name).fetch_one(pool).await.unwrap();
    (attachment, document)
}

async fn extraction(pool: &PgPool, id: i64, status: &str, attempts: i32) {
    sqlx::query("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,attempts,error,next_attempt_at,finished_at)
        SELECT id,sha256,name,$2,$3,$4,CASE WHEN $3='failed' THEN 'Soubor nelze přečíst.' END,now()+interval '30 minutes',now() FROM attachments WHERE id=$1")
        .bind(id).bind(EXTRACTOR_VERSION).bind(status).bind(attempts).execute(pool).await.unwrap();
}

async fn status(app: &App) -> Value {
    let response = app.call("GET", STATUS, None, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    body_json(response).await
}

#[tokio::test]
async fn queue_status_requires_a_live_admin_session_without_extra_permissions() {
    let app = App::new().await;
    let response = app.call("GET", STATUS, None, false).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(body_json(response).await.get("total").is_none());

    let password = "Dalsi-dlouhe-heslo-pro-admina-12345!";
    let id = auth::create_admin(&app.state, "reader@vysker.test", password.into())
        .await
        .unwrap();
    let permissions: (bool,bool,bool,bool,bool) = sqlx::query_as("SELECT is_primary,can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings FROM administrators WHERE id=$1")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(permissions, (false, false, false, false, false));
    let response = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"reader@vysker.test","password":password})),
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let request = || {
        Request::builder()
            .uri(STATUS)
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.router
            .clone()
            .oneshot(request())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    sqlx::query("UPDATE administrators SET active=FALSE WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let response = app.router.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(body_json(response).await.get("total").is_none());
}

#[tokio::test]
async fn queue_status_matches_worker_eligibility_and_treats_stale_results_as_pending() {
    let app = App::new().await;
    let pool = &app.state.pool;
    let (ready, _) = attachment(pool, "hotovo.txt").await;
    extraction(pool, ready, "ready", 1).await;
    attachment(pool, "cekajici.pdf").await;
    let (old_version, _) = attachment(pool, "starsi.docx").await;
    extraction(pool, old_version, "ready", 1).await;
    sqlx::query("UPDATE attachment_extractions SET extractor_version=$2 WHERE attachment_id=$1")
        .bind(old_version)
        .bind(EXTRACTOR_VERSION - 1)
        .execute(pool)
        .await
        .unwrap();
    let (old_name, _) = attachment(pool, "prejmenovany.odt").await;
    extraction(pool, old_name, "failed", 3).await;
    sqlx::query(
        "UPDATE attachment_extractions SET source_name='puvodni.odt' WHERE attachment_id=$1",
    )
    .bind(old_name)
    .execute(pool)
    .await
    .unwrap();
    let (current, current_document) = attachment(pool, "zpracovani.ods").await;
    extraction(pool, current, "processing", 1).await;
    let (retry, _) = attachment(pool, "opakovani.csv").await;
    extraction(pool, retry, "failed", 1).await;
    let (failed, _) = attachment(pool, "chyba.json").await;
    extraction(pool, failed, "failed", 3).await;
    attachment(pool, "tabulka.xlsx").await;
    attachment(pool, "data.XML").await;

    attachment(pool, "nepodporovane.zip").await;
    attachment(pool, "obrazek.png").await;
    let (private, _) = attachment(pool, "soukrome.txt").await;
    sqlx::query("UPDATE attachments SET is_public=FALSE WHERE id=$1")
        .bind(private)
        .execute(pool)
        .await
        .unwrap();
    let (no_data, _) = attachment(pool, "bez-dat.txt").await;
    sqlx::query("UPDATE attachments SET data=NULL WHERE id=$1")
        .bind(no_data)
        .execute(pool)
        .await
        .unwrap();
    let (removed, _) = attachment(pool, "odebrany.pdf").await;
    sqlx::query("UPDATE attachments SET data=NULL,removed_at=now() WHERE id=$1")
        .bind(removed)
        .execute(pool)
        .await
        .unwrap();

    let value = status(&app).await;
    for (field, expected) in [
        ("total", 9),
        ("ready", 1),
        ("pending", 5),
        ("processing", 1),
        ("failed", 2),
        ("retrying", 1),
    ] {
        assert_eq!(value[field], expected, "{field}");
    }
    assert_eq!(
        value["current"],
        json!({"attachment_id":current,"document_id":current_document,"name":"zpracovani.ods"})
    );
    let failed_items = value["failed_items"].as_array().unwrap();
    assert_eq!(failed_items.len(), 2);
    assert!(
        failed_items
            .iter()
            .all(|item| [retry, failed].contains(&item["attachment_id"].as_i64().unwrap()))
    );
    assert!(
        failed_items
            .iter()
            .all(|item| item["error"] == "Soubor nelze přečíst.")
    );
}

#[tokio::test]
async fn queue_status_handles_empty_queue_and_bounds_failure_details() {
    let app = App::new().await;
    assert_eq!(
        status(&app).await,
        json!({"total":0,"ready":0,"pending":0,"processing":0,"failed":0,"retrying":0,"current":null,"failed_items":[]})
    );
    let pool = &app.state.pool;
    let mut ids = Vec::new();
    for n in 0..8 {
        let (id, _) = attachment(pool, &format!("chyba-{n}.pdf")).await;
        extraction(pool, id, "failed", 3).await;
        sqlx::query("UPDATE attachment_extractions SET finished_at='2026-11-01T12:00:00Z',error=repeat('ž',1200) WHERE attachment_id=$1").bind(id).execute(pool).await.unwrap();
        ids.push(id);
    }
    let value = status(&app).await;
    assert_eq!(value["total"], 8);
    assert_eq!(value["failed"], 8);
    assert_eq!(value["retrying"], 0);
    assert!(value["current"].is_null());
    let items = value["failed_items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(
        items
            .iter()
            .map(|item| item["attachment_id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        ids.into_iter().rev().take(5).collect::<Vec<_>>()
    );
    assert!(
        items
            .iter()
            .all(|item| item["error"].as_str().unwrap().chars().count() == 1000)
    );
}
