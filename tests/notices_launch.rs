#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::*;
use obecni_web::{
    backend::{self, notices},
    model,
};
use serde_json::json;
use time::{Duration, OffsetDateTime, macros::datetime};

#[tokio::test]
async fn publication_is_not_backdated_and_live_content_is_immutable() {
    let app = App::new().await;
    let yesterday = backend::today(OffsetDateTime::now_utc()) - Duration::days(1);
    let id = app.notice(json!({"published_on":yesterday})).await;
    assert_eq!(app.publish(id).await.status(), StatusCode::BAD_REQUEST);
    let id = app.notice(json!({})).await;
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/notices/{id}"),
            Some(json!({"title":"Dodatečná změna","retain_attachments":false})),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let proof = body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/notices/{id}/evidence"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert!(proof["notice"]["published_at"].is_string());
    assert_eq!(proof["events"][0]["kind"], "published");
    assert!(
        sqlx::query("DELETE FROM notice_events")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/admin/notices/{id}/evidence"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn statutory_minimum_blocks_early_removal_except_documented_incident() {
    let app = App::new().await;
    let id = app.notice(json!({"review":{"rule":"public_notice"}})).await;
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    let path = format!("/api/v1/admin/notices/{id}/withdraw");
    assert_eq!(
        app.call("POST", &path, None, true).await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(json!({"emergency":true,"reason":""})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(json!({"emergency":true,"reason":"Chybně zveřejněná příloha"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let proof = body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/notices/{id}/evidence"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert!(
        proof["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "emergency_withdrawal")
    );
}

#[tokio::test]
async fn visibility_revocation_hides_detail_and_files_but_preserves_admin_original() {
    let app = App::new().await;
    let id=app.notice(json!({"title":"Původní název","description":"Původní popis","reference_number":"Původní reference","issuer":"Původce"})).await;
    let file = body_json(
        app.upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "original.pdf",
            PDF,
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/notices/{id}/withdraw"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    // Hiding attachments after removal leaves the independent public metadata intact.
    let detail = body_json(
        app.call("GET", &format!("/api/v1/notices/{id}"), None, false)
            .await,
    )
    .await;
    assert_eq!(detail["title"], "Původní název");
    assert_eq!(detail["status"], "archived");
    assert!(detail["attachments"].as_array().unwrap().is_empty());
    let row = model::notices::by_id(&app.state.pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.title, "Původní název");
    assert!(
        model::notices::attachments(&app.state.pool, id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/archive"),
            Some(json!({"reason":"Okamžité ukončení veřejného přístupu"})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    for path in [
        format!("/api/v1/notices/{id}"),
        format!("/api/v1/documents/{id}"),
        format!("/api/v1/attachments/{file}"),
    ] {
        let response = app.call("GET", &path, None, false).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    assert!(
        model::notices::by_id(&app.state.pool, id)
            .await
            .unwrap()
            .is_none()
    );
    let admin = body_json(
        app.call("GET", &format!("/api/v1/admin/notices/{id}"), None, true)
            .await,
    )
    .await;
    assert_eq!(admin["title"], "Původní název");
    let preserved: Vec<u8> = sqlx::query_scalar("SELECT data FROM attachments WHERE id=$1")
        .bind(file)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(preserved, PDF);
}

#[tokio::test]
async fn retained_files_require_justification_and_expire() {
    let app = App::new().await;
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/notices",
            Some(json!({"title":"Neomezený archiv","retain_attachments":true})),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let until = backend::today(OffsetDateTime::now_utc()) + Duration::days(60);
    let id=app.notice(json!({"retain_attachments":true,"review":{"archive_basis":"Zveřejnění obecného dokumentu bez osobních údajů","archive_until":until}})).await;
    let file = body_json(
        app.upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "rozpocet.pdf",
            PDF,
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    app.publish(id).await;
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/notices/{id}/withdraw"),
            None,
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
    sqlx::query("UPDATE documents SET posting_public_until='2020-01-01' WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    notices::maintenance(&app.state, OffsetDateTime::now_utc())
        .await
        .unwrap();
    let preserved: bool =
        sqlx::query_scalar("SELECT data IS NOT NULL FROM attachments WHERE id=$1")
            .bind(file)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(preserved);
}

#[tokio::test]
async fn missed_start_returns_to_draft_without_claiming_past_publication() {
    let app = App::new().await;
    let id = app
        .notice(json!({"published_on":"2026-10-01","withdraw_on":"2026-11-01"}))
        .await;
    notices::publish_at(
        &app.state,
        id,
        app.admin_id,
        datetime!(2026-09-30 12:00 UTC),
    )
    .await
    .unwrap();
    notices::maintenance(&app.state, datetime!(2026-10-02 12:00 UTC))
        .await
        .unwrap();
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status,published_at FROM notices WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(row, ("draft".into(), None));
    let event: String = sqlx::query_scalar("SELECT kind FROM notice_events WHERE notice_id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(event, "schedule_missed");
}

async fn assert_unpublished_draft(app: &App, id: i64, file: i64, operation: &str) {
    let row: (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT status,published_at,withdrawn_at FROM notices WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(row, ("draft".into(), None, None));
    for path in [
        format!("/api/v1/notices/{id}"),
        format!("/api/v1/attachments/{file}"),
    ] {
        assert_eq!(
            app.call("GET", &path, None, false).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let archived = body_json(
        app.call("GET", "/api/v1/notices?archived=true", None, false)
            .await,
    )
    .await;
    assert!(archived.as_array().unwrap().is_empty());
    assert!(
        model::notices::by_id(&app.state.pool, id)
            .await
            .unwrap()
            .is_none()
    );
    let data: Option<Vec<u8>> = sqlx::query_scalar("SELECT data FROM attachments WHERE id=$1")
        .bind(file)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(data.as_deref(), Some(PDF));
    let events: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM notice_events WHERE notice_id=$1 ORDER BY id")
            .bind(id)
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(events, [operation]);
}

#[tokio::test]
async fn cancelling_a_schedule_preserves_private_attachments_without_a_public_archive() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let posted = backend::today(now) + Duration::days(1);
    for retain in [false, true] {
        let id = app
            .notice(json!({
                "published_on":posted,
                "retain_attachments":retain,
                "review":{
                    "rule":"public_notice",
                    "archive_title":"Dosud nezveřejněný záznam",
                    "archive_basis":"Obecný dokument",
                    "archive_until":posted + Duration::days(60)
                }
            }))
            .await;
        let file = body_json(
            app.upload(
                &format!("/api/v1/admin/notices/{id}/attachments"),
                "soukromy-koncept.pdf",
                PDF,
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        notices::publish_at(&app.state, id, app.admin_id, now)
            .await
            .unwrap();
        assert_eq!(
            app.call(
                "POST",
                &format!("/api/v1/admin/notices/{id}/withdraw"),
                Some(json!({"expected_status":"scheduled"})),
                true
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        notices::maintenance(&app.state, now).await.unwrap();
        assert_unpublished_draft(&app, id, file, "schedule_cancelled").await;
    }
}

#[tokio::test]
async fn stale_schedule_cancellation_cannot_withdraw_a_published_notice() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let id = app.notice(json!({})).await;
    let file = body_json(
        app.upload(
            &format!("/api/v1/admin/notices/{id}/attachments"),
            "zverejneny-dokument.pdf",
            PDF,
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    notices::publish_at(&app.state, id, app.admin_id, now - Duration::days(1))
        .await
        .unwrap();
    let scheduled = body_json(
        app.call("GET", &format!("/api/v1/admin/notices/{id}"), None, true)
            .await,
    )
    .await;
    assert_eq!(scheduled["status"], "scheduled");
    notices::maintenance(&app.state, now).await.unwrap();
    let proof_path = format!("/api/v1/admin/notices/{id}/evidence");
    let before = body_json(app.call("GET", &proof_path, None, true).await).await;
    assert_eq!(before["notice"]["status"], "published");
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/notices/{id}/withdraw"),
            Some(json!({"expected_status":scheduled["status"]})),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let after = body_json(app.call("GET", &proof_path, None, true).await).await;
    assert_eq!(after, before);
    assert_eq!(
        app.call("GET", &format!("/api/v1/attachments/{file}"), None, false)
            .await
            .status(),
        StatusCode::OK
    );
    let data: Option<Vec<u8>> = sqlx::query_scalar("SELECT data FROM attachments WHERE id=$1")
        .bind(file)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(data.as_deref(), Some(PDF));
}

#[tokio::test]
async fn missing_the_entire_scheduled_period_keeps_the_notice_and_its_files_private() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let today = backend::today(now);
    for retain in [false, true] {
        let id = app
            .notice(json!({
                "published_on":today - Duration::days(2),
                "withdraw_on":today - Duration::days(1),
                "retain_attachments":retain,
                "review":{
                    "archive_title":"Dosud nezveřejněný záznam",
                    "archive_basis":"Obecný dokument",
                    "archive_until":today + Duration::days(30)
                }
            }))
            .await;
        let file = body_json(
            app.upload(
                &format!("/api/v1/admin/notices/{id}/attachments"),
                "soukromy-koncept.pdf",
                PDF,
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        notices::publish_at(&app.state, id, app.admin_id, now - Duration::days(3))
            .await
            .unwrap();
        notices::maintenance(&app.state, now).await.unwrap();
        notices::maintenance(&app.state, now).await.unwrap();
        assert_unpublished_draft(&app, id, file, "schedule_missed").await;
    }
}

#[tokio::test]
async fn production_requires_review_and_reference_to_original() {
    let app = App::new().await;
    let id = app.notice(json!({})).await;
    let mut config = (*app.state.config).clone();
    config.production = true;
    let prod = backend::Backend::new(app.state.pool.clone(), config);
    assert_eq!(
        notices::publish_at(&prod, id, app.admin_id, OffsetDateTime::now_utc())
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let id=app.notice(json!({"review":{"rule":"informational","legal_basis":"Obecní informace","original_reference":"spis-2026-1","reviewed":true}})).await;
    notices::publish_at(&prod, id, app.admin_id, OffsetDateTime::now_utc())
        .await
        .unwrap();
}
