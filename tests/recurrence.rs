#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::{
    backend::events::{public_event_from_pool_at, public_events_from_pool},
    events::Event,
};
use serde_json::{Value, json};
use time::{
    Date, Duration, OffsetDateTime, format_description::well_known::Rfc3339, macros::datetime,
};

fn input(extra: Value) -> Value {
    let mut value = json!({
        "title":"Svoz odpadu", "category":"waste", "published":true,
        "starts_at":"2026-03-15T08:00", "ends_at":"2026-03-15T10:00",
        "repeat_every_14_days":true
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

async fn create(app: &App, extra: Value) -> Event {
    let response = app
        .call("POST", "/api/v1/admin/events", Some(input(extra)), true)
        .await;
    let status = response.status();
    let body = body_json(response).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn occurrence(app: &App, id: i64, now: OffsetDateTime) -> Event {
    public_event_from_pool_at(&app.state.pool, id, now)
        .await
        .unwrap()
        .unwrap()
}

fn instant(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap()
}

#[tokio::test]
async fn recurrence_validates_bounds_precision_and_round_trips_optional_end() {
    let app = App::new().await;
    for extra in [
        json!({"repeat_every_14_days":false,"repeat_until":"2026-03-29"}),
        json!({"repeat_until":"2026-03-14"}),
        json!({"repeat_until":"2026-02-30"}),
        json!({"repeat_until":""}),
        json!({"repeat_until":"2026-03-29T10:00"}),
        json!({"ends_at":"2026-03-29T08:00"}),
        json!({"all_day":true,"starts_at":"2026-03-15","ends_at":"2026-03-28"}),
        json!({"starts_at":"1995-03-15T08:00","ends_at":"1995-03-15T09:00"}),
        json!({"starts_at":"2100-03-15T08:00","ends_at":"2100-03-15T09:00"}),
        json!({"repeat_until":"2100-03-29"}),
        json!({"starts_at":"2026-03-15T08:00:00.1"}),
        json!({"starts_at":"2026-10-25T02:30:00+01:00","ends_at":"2026-10-25T03:30:00+01:00"}),
    ] {
        let response = app
            .call(
                "POST",
                "/api/v1/admin/events",
                Some(input(extra.clone())),
                true,
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{extra}");
    }
    let event = create(&app, json!({"repeat_until":"2026-03-29"})).await;
    assert!(event.repeat_every_14_days);
    assert_eq!(event.repeat_until.as_deref(), Some("2026-03-29"));
    assert!(event.occurrence_starts_at.is_none());
    assert!(event.recurrence_label().unwrap().contains("29. 3. 2026"));
    let path = format!("/api/v1/admin/events/{}", event.id);
    let response = app
        .call(
            "PUT",
            &path,
            Some(input(json!({
                "expected_version":event.version,"repeat_every_14_days":false,"repeat_until":null
            }))),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let event: Event = serde_json::from_value(body_json(response).await).unwrap();
    assert!(!event.repeat_every_14_days);
    assert_eq!(event.repeat_until, None);
    assert_eq!(event.version, 2);
    assert!(event.recurrence_label().is_none());
}

#[tokio::test]
async fn public_projection_preserves_anchors_and_orders_before_pagination() {
    let app = App::new().await;
    let series = create(&app, json!({})).await;
    let single = create(
        &app,
        json!({
            "repeat_every_14_days":false,"starts_at":"2026-03-25T08:00","ends_at":"2026-03-25T09:00"
        }),
    )
    .await;
    let draft = create(&app, json!({"published":false})).await;
    let now = datetime!(2026-03-20 12:00 UTC);
    let page = public_events_from_pool(&app.state.pool, false, 1, 0, now)
        .await
        .unwrap();
    assert_eq!(page[0].id, single.id);
    let page = public_events_from_pool(&app.state.pool, false, 1, 1, now)
        .await
        .unwrap();
    assert_eq!(page[0].id, series.id);
    assert_eq!(page[0].display_start(), "2026-03-29T08:00:00+02:00");
    assert_eq!(page[0].starts_at, "2026-03-15T08:00:00+01:00");
    assert_eq!(occurrence(&app, series.id, now).await, page[0]);
    assert!(
        public_events_from_pool(&app.state.pool, true, 20, 0, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        public_event_from_pool_at(&app.state.pool, draft.id, now)
            .await
            .unwrap()
            .is_none()
    );
    let ongoing = occurrence(&app, series.id, datetime!(2026-03-29 7:00 UTC)).await;
    assert_eq!(ongoing.display_start(), "2026-03-29T08:00:00+02:00");
    let following = occurrence(&app, series.id, datetime!(2026-03-29 8:00:01 UTC)).await;
    assert_eq!(following.display_start(), "2026-04-12T08:00:00+02:00");
    let admin: Event = serde_json::from_value(
        body_json(
            app.call(
                "GET",
                &format!("/api/v1/admin/events/{}", series.id),
                None,
                true,
            )
            .await,
        )
        .await,
    )
    .unwrap();
    assert_eq!(admin, series);
}

#[tokio::test]
async fn finite_all_day_series_archives_after_inclusive_last_occurrence_ends() {
    let app = App::new().await;
    let series = create(
        &app,
        json!({
            "all_day":true,"starts_at":"2026-03-15","ends_at":"2026-03-16",
            "repeat_until":"2026-03-29","cancelled":true
        }),
    )
    .await;
    let last_end = datetime!(2026-03-30 21:59:59 UTC);
    let upcoming = public_events_from_pool(&app.state.pool, false, 20, 0, last_end)
        .await
        .unwrap();
    assert_eq!(upcoming.len(), 1);
    assert!(upcoming[0].cancelled);
    assert_eq!(upcoming[0].display_start(), "2026-03-29T00:00:00+01:00");
    assert_eq!(upcoming[0].display_end(), "2026-03-30T23:59:59+02:00");
    assert!(
        public_events_from_pool(&app.state.pool, true, 20, 0, last_end)
            .await
            .unwrap()
            .is_empty()
    );
    let after = last_end + Duration::seconds(1);
    assert!(
        public_events_from_pool(&app.state.pool, false, 20, 0, after)
            .await
            .unwrap()
            .is_empty()
    );
    let archived = public_events_from_pool(&app.state.pool, true, 20, 0, after)
        .await
        .unwrap();
    assert_eq!(archived, upcoming);
    assert_eq!(occurrence(&app, series.id, after).await, archived[0]);
}

#[tokio::test]
async fn nonexistent_starts_are_skipped_ambiguous_starts_choose_first_and_duration_is_exact() {
    let app = App::new().await;
    let gap = create(
        &app,
        json!({"starts_at":"2026-03-15T02:30","ends_at":"2026-03-15T03:30"}),
    )
    .await;
    assert_eq!(
        occurrence(&app, gap.id, datetime!(2026-03-20 0:00 UTC))
            .await
            .display_start(),
        "2026-04-12T02:30:00+02:00"
    );
    let finite_gap = create(&app, json!({"starts_at":"2026-03-15T02:30","ends_at":"2026-03-15T03:30","repeat_until":"2026-03-29"})).await;
    assert_eq!(
        occurrence(&app, finite_gap.id, datetime!(2026-04-01 0:00 UTC))
            .await
            .display_start(),
        finite_gap.starts_at
    );
    let fold = create(
        &app,
        json!({"starts_at":"2026-10-11T02:30","ends_at":"2026-10-11T03:30"}),
    )
    .await;
    let folded = occurrence(&app, fold.id, datetime!(2026-10-20 0:00 UTC)).await;
    assert_eq!(folded.display_start(), "2026-10-25T02:30:00+02:00");
    assert_eq!(folded.display_end(), "2026-10-25T02:30:00+01:00");
    let crosses_dst = create(
        &app,
        json!({"starts_at":"2026-03-28T20:00","ends_at":"2026-03-29T10:00"}),
    )
    .await;
    let next = occurrence(&app, crosses_dst.id, datetime!(2026-04-01 0:00 UTC)).await;
    assert_eq!(next.display_start(), "2026-04-11T20:00:00+02:00");
    assert_eq!(next.display_end(), "2026-04-12T09:00:00+02:00");
    assert_eq!(
        instant(next.display_end()) - instant(next.display_start()),
        Duration::hours(13)
    );
}

#[tokio::test]
async fn unknown_imported_times_require_confirmation_before_recurrence() {
    let app = App::new().await;
    let event = create(&app, json!({"repeat_every_14_days":false})).await;
    let version: i64 = sqlx::query_scalar("UPDATE events SET start_time_known=FALSE,end_time_known=FALSE,end_date_known=FALSE WHERE id=$1 RETURNING version")
        .bind(event.id).fetch_one(&app.state.pool).await.unwrap();
    let path = format!("/api/v1/admin/events/{}", event.id);
    assert_eq!(
        app.call(
            "PUT",
            &path,
            Some(input(json!({"expected_version":version}))),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = app.call("PUT", &path, Some(input(json!({
        "expected_version":version,"starts_at":"2026-03-15T09:00","ends_at":"2026-03-15T11:00"
    }))), true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let precise: Event = serde_json::from_value(body_json(response).await).unwrap();
    assert!(precise.repeat_every_14_days);
    assert!(precise.start_time_known && precise.end_time_known && precise.end_date_known);
    let version = precise.version;
    let response = app.call("PUT", &path, Some(input(json!({
        "expected_version":version,"all_day":true,"starts_at":"2026-03-15","ends_at":"2026-03-15"
    }))), true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: Event = serde_json::from_value(body_json(response).await).unwrap();
    assert!(updated.repeat_every_14_days);
    assert!(updated.all_day);
    assert!(updated.start_time_known && updated.end_time_known && updated.end_date_known);
}

#[tokio::test]
async fn direct_recurrence_updates_advance_revision_and_withdrawals_keep_only_old_metadata() {
    let app = App::new().await;
    let series = create(&app, json!({"repeat_until":"2026-03-29"})).await;
    let version: i64 = sqlx::query_scalar(
        "UPDATE events SET repeat_until='2026-04-12' WHERE id=$1 RETURNING version",
    )
    .bind(series.id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(version, series.version + 1);
    sqlx::query("UPDATE events SET category='other',repeat_until='2026-04-26' WHERE id=$1")
        .bind(series.id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let withdrawn: (bool, Option<Date>, i64) = sqlx::query_as("SELECT repeat_every_14_days,repeat_until,version FROM calendar_withdrawals WHERE category='waste'")
        .fetch_one(&app.state.pool).await.unwrap();
    assert!(withdrawn.0);
    assert_eq!(withdrawn.1.unwrap().to_string(), "2026-04-12");
    assert_eq!(withdrawn.2, version + 1);
    sqlx::query("DELETE FROM events WHERE id=$1")
        .bind(series.id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let removed: (bool, Option<Date>, i64) = sqlx::query_as("SELECT repeat_every_14_days,repeat_until,version FROM calendar_withdrawals WHERE category='*'")
        .fetch_one(&app.state.pool).await.unwrap();
    assert!(removed.0);
    assert_eq!(removed.1.unwrap().to_string(), "2026-04-26");
    assert_eq!(removed.2, version + 2);
}
