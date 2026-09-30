#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{App, body_json};
use obecni_web::{
    backend::events::{parse_event_datetime, public_events_from_pool},
    events::Event,
};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339, macros::datetime};
use tower::ServiceExt;

fn input() -> Value {
    json!({"title":"Setkání sousedů","description":"Program pro občany.","location":"Náves","starts_at":"2030-07-10T12:00","ends_at":"2030-07-10T14:00","published":false,"cancelled":false})
}

async fn create(app: &App, extra: Value) -> Event {
    let mut value = input();
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let response = app
        .call("POST", "/api/v1/admin/events", Some(value), true)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    serde_json::from_value(body_json(response).await).unwrap()
}

fn iso(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap()
}

#[test]
fn datetime_local_uses_prague_and_rejects_dst_gaps_and_ambiguities() {
    assert_eq!(
        parse_event_datetime("2026-07-10T12:00").unwrap(),
        datetime!(2026-07-10 10:00 UTC)
    );
    assert_eq!(
        parse_event_datetime("2026-01-10T12:00").unwrap(),
        datetime!(2026-01-10 11:00 UTC)
    );
    assert!(parse_event_datetime("2026-03-29T02:30").is_err());
    assert!(parse_event_datetime("2026-10-25T02:30").is_err());
    let summer = parse_event_datetime("2026-10-25T02:30:00+02:00").unwrap();
    let winter = parse_event_datetime("2026-10-25T02:30:00+01:00").unwrap();
    assert_eq!(winter - summer, Duration::hours(1));
    assert_eq!(
        parse_event_datetime("2026-03-29T02:30:00+01:00").unwrap(),
        datetime!(2026-03-29 1:30 UTC)
    );
    assert_eq!(
        parse_event_datetime("2026-07-10T12:00:34.123456")
            .unwrap()
            .microsecond(),
        123456
    );
    for value in [
        "",
        "2026-02-30T12:00",
        "2026-07-10",
        "2026-07-10T24:00",
        "2026-07-10T12:00:00+25:00",
        "0000-01-01T12:00",
        "9999-12-31T23:30:00Z",
        "9999-12-31T23:30:00-10:00",
        "1800-01-01T12:00:00Z",
    ] {
        assert!(parse_event_datetime(value).is_err(), "{value}");
    }
}

#[tokio::test]
async fn every_admin_endpoint_requires_auth_and_mutations_require_csrf_and_same_origin() {
    let app = App::new().await;
    for (method, path, body) in [
        ("GET", "/api/v1/admin/events", None),
        ("GET", "/api/v1/admin/events/1", None),
        ("POST", "/api/v1/admin/events", Some(input())),
        ("PUT", "/api/v1/admin/events/1", Some(input())),
        (
            "DELETE",
            "/api/v1/admin/events/1",
            Some(json!({"expected_version":1})),
        ),
    ] {
        assert_eq!(
            app.call(method, path, body, false).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for (method, path, body) in [
        ("POST", "/api/v1/admin/events", input()),
        ("PUT", "/api/v1/admin/events/1", input()),
        (
            "DELETE",
            "/api/v1/admin/events/1",
            json!({"expected_version":1}),
        ),
    ] {
        for hostile_origin in [false, true] {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", &app.cookie)
                .header("content-type", "application/json");
            if hostile_origin {
                request = request
                    .header("x-csrf-token", &app.csrf)
                    .header("origin", "https://unrelated.example");
            }
            let response = app
                .router
                .clone()
                .oneshot(request.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn crud_preserves_normalized_text_dates_versions_and_audit() {
    let app = App::new().await;
    let created=create(&app,json!({"title":"  Setkání sousedů  ","description":"První řádek\r\nDruhý řádek","location":"  Náves  "})).await;
    assert_eq!(created.title, "Setkání sousedů");
    assert_eq!(created.location, "Náves");
    assert_eq!(created.description, "První řádek\nDruhý řádek");
    assert_eq!(created.version, 1);
    assert_eq!(created.starts_at, "2030-07-10T12:00:00+02:00");
    assert_eq!(created.href(), format!("/kalendar/{}", created.id));
    let path = format!("/api/v1/admin/events/{}", created.id);
    let public_path = format!("/api/v1/events/{}", created.id);
    assert_eq!(
        app.call("GET", &public_path, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    let stored: Event =
        serde_json::from_value(body_json(app.call("GET", &path, None, true).await).await).unwrap();
    assert_eq!(stored, created);
    let list = body_json(app.call("GET", "/api/v1/admin/events", None, true).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let mut update = input();
    update["expected_version"] = json!(1);
    update["published"] = json!(true);
    update["cancelled"] = json!(true);
    let response = app.call("PUT", &path, Some(update), true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: Event = serde_json::from_value(body_json(response).await).unwrap();
    assert_eq!(updated.version, 2);
    assert!(updated.published && updated.cancelled);
    let public = body_json(app.call("GET", &public_path, None, false).await).await;
    assert_eq!(public["cancelled"], true);
    assert_eq!(public["id"], created.id);
    assert_eq!(
        app.call("DELETE", &path, Some(json!({"expected_version":2})), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", &path, None, true).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call("GET", &public_path, None, false).await.status(),
        StatusCode::NOT_FOUND
    );
    let audit:Vec<(String,Option<i64>)>=sqlx::query_as("SELECT operation,actor_id FROM audit_log WHERE entity_type='event' AND entity_id=$1 ORDER BY id")
        .bind(created.id).fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(
        audit,
        vec![
            ("created".into(), Some(app.admin_id)),
            ("updated".into(), Some(app.admin_id)),
            ("deleted".into(), Some(app.admin_id))
        ]
    );
}

#[tokio::test]
async fn validation_rejects_invalid_fields_dates_and_versions_without_writes() {
    let app = App::new().await;
    let invalid = [
        json!({"title":" \t "}),
        json!({"title":"ž".repeat(201)}),
        json!({"title":"Akce\nDalší"}),
        json!({"location":"ž".repeat(301)}),
        json!({"location":"Náves\u{0000}"}),
        json!({"description":"ž".repeat(10_001)}),
        json!({"description":"Text\u{0007}"}),
        json!({"starts_at":"neplatné datum"}),
        json!({"ends_at":"2030-07-10T11:59"}),
        json!({"starts_at":"2026-03-29T02:30"}),
        json!({"starts_at":"2026-10-25T02:30"}),
        json!({"expected_version":1}),
    ];
    for extra in invalid {
        let mut value = input();
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            app.call("POST", "/api/v1/admin/events", Some(value.clone()), true)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{extra}"
        );
    }
    let mut extra_field = input();
    extra_field["unexpected"] = json!(true);
    assert_eq!(
        app.call("POST", "/api/v1/admin/events", Some(extra_field), true)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let created=create(&app,json!({"title":"ž".repeat(200),"location":"ž".repeat(300),"description":"ž".repeat(10_000),"starts_at":"2026-10-25T02:30:00+02:00","ends_at":"2026-10-25T02:30:00+01:00"})).await;
    assert_eq!(created.title.chars().count(), 200);
    let path = format!("/api/v1/admin/events/{}", created.id);
    for version in [Value::Null, json!(0), json!(-1), json!(i64::MAX)] {
        let mut value = input();
        value["expected_version"] = version.clone();
        assert_eq!(
            app.call("PUT", &path, Some(value), true).await.status(),
            StatusCode::BAD_REQUEST
        );
        let deletion = app
            .call(
                "DELETE",
                &path,
                Some(json!({"expected_version":version})),
                true,
            )
            .await;
        assert!(matches!(
            deletion.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ));
    }
    let mut reversed = input();
    reversed["expected_version"] = json!(1);
    reversed["ends_at"] = json!("2030-07-10T11:59");
    assert_eq!(
        app.call("PUT", &path, Some(reversed), true).await.status(),
        StatusCode::BAD_REQUEST
    );
    let after: Event =
        serde_json::from_value(body_json(app.call("GET", &path, None, true).await).await).unwrap();
    assert_eq!(after, created);
    for query in ["limit=0", "limit=101", "offset=-1", "offset=100001"] {
        for route in ["/api/v1/events", "/api/v1/admin/events"] {
            assert_eq!(
                app.call("GET", &format!("{route}?{query}"), None, true)
                    .await
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
    }
}

#[tokio::test]
async fn public_visibility_upcoming_archive_pagination_and_permanent_details_share_rules() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let past = |cancelled| json!({"starts_at":iso(now-Duration::days(2)),"ends_at":iso(now-Duration::days(1)),"published":true,"cancelled":cancelled});
    let future = |cancelled| json!({"starts_at":iso(now+Duration::days(1)),"ends_at":iso(now+Duration::days(1)+Duration::hours(2)),"published":true,"cancelled":cancelled});
    let archived = create(&app, past(false)).await;
    let cancelled_past = create(&app, past(true)).await;
    let ongoing=create(&app,json!({"starts_at":iso(now-Duration::hours(1)),"ends_at":iso(now+Duration::hours(1)),"published":true})).await;
    let first = create(&app, future(false)).await;
    let cancelled_future = create(&app, future(true)).await;
    let mut hidden = future(true);
    hidden["published"] = json!(false);
    let draft = create(&app, hidden).await;
    let mut hidden_past = past(true);
    hidden_past["published"] = json!(false);
    let draft_past = create(&app, hidden_past).await;
    let upcoming = public_events_from_pool(&app.state.pool, false, 2, 0, now)
        .await
        .unwrap();
    assert_eq!(
        upcoming.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![ongoing.id, first.id]
    );
    let second = public_events_from_pool(&app.state.pool, false, 2, 2, now)
        .await
        .unwrap();
    assert_eq!(
        second.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![cancelled_future.id]
    );
    assert!(second[0].cancelled);
    let archive = public_events_from_pool(&app.state.pool, true, 20, 0, now)
        .await
        .unwrap();
    assert_eq!(
        archive.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![cancelled_past.id, archived.id]
    );
    let public: Vec<Event> = serde_json::from_value(
        body_json(
            app.call("GET", "/api/v1/events?limit=2&offset=2", None, false)
                .await,
        )
        .await,
    )
    .unwrap();
    assert_eq!(public, second);
    let public_archive: Vec<Event> = serde_json::from_value(
        body_json(
            app.call("GET", "/api/v1/events?archived=true", None, false)
                .await,
        )
        .await,
    )
    .unwrap();
    assert_eq!(public_archive, archive);
    for event in [
        &archived,
        &cancelled_past,
        &ongoing,
        &first,
        &cancelled_future,
    ] {
        let response = app
            .call("GET", &format!("/api/v1/events/{}", event.id), None, false)
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["cancelled"], event.cancelled);
    }
    for id in [draft.id, draft_past.id, 999_999] {
        assert_eq!(
            app.call("GET", &format!("/api/v1/events/{id}"), None, false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let boundary = create(
        &app,
        json!({"starts_at":iso(now),"ends_at":iso(now),"published":true}),
    )
    .await;
    let at_end = public_events_from_pool(&app.state.pool, false, 100, 0, now)
        .await
        .unwrap();
    assert!(at_end.iter().any(|e| e.id == boundary.id));
    let after_end = public_events_from_pool(
        &app.state.pool,
        true,
        100,
        0,
        now + Duration::microseconds(1),
    )
    .await
    .unwrap();
    assert!(after_end.iter().any(|e| e.id == boundary.id));
}

#[tokio::test]
async fn simultaneous_updates_have_one_winner_and_stale_delete_cannot_remove_it() {
    let app = App::new().await;
    let event = create(&app, json!({})).await;
    let path = format!("/api/v1/admin/events/{}", event.id);
    let mut first = input();
    first["expected_version"] = json!(1);
    first["title"] = json!("Úprava prvního správce");
    let mut second = first.clone();
    second["title"] = json!("Úprava druhého správce");
    let (a, b) = tokio::join!(
        app.call("PUT", &path, Some(first), true),
        app.call("PUT", &path, Some(second), true)
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let current = body_json(app.call("GET", &path, None, true).await).await;
    assert_eq!(current["version"], 2);
    assert!(
        current["title"] == "Úprava prvního správce"
            || current["title"] == "Úprava druhého správce"
    );
    assert_eq!(
        app.call("DELETE", &path, Some(json!({"expected_version":1})), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        body_json(app.call("GET", &path, None, true).await).await,
        current
    );
    let changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE entity_type='event' AND operation='updated'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(changes, 1);
}

#[tokio::test]
async fn simultaneous_update_and_delete_never_both_succeed() {
    let app = App::new().await;
    let event = create(&app, json!({})).await;
    let path = format!("/api/v1/admin/events/{}", event.id);
    let mut update = input();
    update["expected_version"] = json!(1);
    let (updated, deleted) = tokio::join!(
        app.call("PUT", &path, Some(update), true),
        app.call("DELETE", &path, Some(json!({"expected_version":1})), true)
    );
    assert_ne!(updated.status().is_success(), deleted.status().is_success());
    if updated.status().is_success() {
        assert_eq!(deleted.status(), StatusCode::CONFLICT);
    } else {
        assert_eq!(updated.status(), StatusCode::NOT_FOUND);
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE entity_type='event' AND operation IN ('updated','deleted')").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn failed_audit_rolls_back_the_event_write() {
    let app = App::new().await;
    sqlx::raw_sql("CREATE FUNCTION reject_event_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.entity_type='event' THEN RAISE EXCEPTION 'Synthetic audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_event_audit BEFORE INSERT ON audit_log FOR EACH ROW EXECUTE FUNCTION reject_event_audit();")
        .execute(&app.state.pool).await.unwrap();
    assert_eq!(
        app.call("POST", "/api/v1/admin/events", Some(input()), true)
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
