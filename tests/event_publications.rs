#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::{
    backend::{
        self, Backend, cez, mail,
        power_outages::{OutageSource, sync_if_due},
        subscriptions,
    },
    events::Event,
};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

async fn subscriber(app: &App, email: &str, preferences: Option<Value>, confirm: bool) -> i64 {
    let mut input = json!({"email":email});
    if let Some(preferences) = preferences {
        input["preferences"] = preferences;
    }
    let response = app
        .call("POST", "/api/v1/subscriptions", Some(input), false)
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    if confirm {
        subscriptions::use_token(
            &app.state,
            &app.confirmation_token(email).await,
            true,
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    }
    sqlx::query_scalar("SELECT id FROM subscribers WHERE email=$1")
        .bind(email)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

fn input() -> Value {
    json!({
        "title":"Setkání sousedů", "description":"Program pro občany.",
        "location":"Náves", "starts_at":"2030-07-10T12:00",
        "ends_at":"2030-07-10T14:00", "published":false, "cancelled":false,
    })
}

async fn create(app: &App, extra: Value) -> Event {
    let mut input = input();
    input
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let response = app
        .call("POST", "/api/v1/admin/events", Some(input), true)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    serde_json::from_value(body_json(response).await).unwrap()
}

fn update_input(event: &Event, extra: Value) -> Value {
    let mut input = json!({
        "title":event.title, "description":event.description, "location":event.location,
        "starts_at":event.starts_at, "ends_at":event.ends_at,
        "published":event.published, "cancelled":event.cancelled,
        "category":event.category, "all_day":event.all_day,
        "repeat_every_14_days":event.repeat_every_14_days, "repeat_until":event.repeat_until,
        "expected_version":event.version,
    });
    input
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    input
}

async fn update(app: &App, event: &Event, extra: Value) -> Event {
    let response = app
        .call(
            "PUT",
            &format!("/api/v1/admin/events/{}", event.id),
            Some(update_input(event, extra)),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_value(body_json(response).await).unwrap()
}

async fn recipients(app: &App, event: i64) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT subscriber_id FROM publication_outbox WHERE event_id=$1 ORDER BY subscriber_id",
    )
    .bind(event)
    .fetch_all(&app.state.pool)
    .await
    .unwrap()
}

async fn announced_at(app: &App, event: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT email_announced_at FROM events WHERE id=$1")
        .bind(event)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn prepare_all(app: &App, now: OffsetDateTime) {
    for _ in 0..20 {
        if !mail::prepare_publications(&app.state, now).await.unwrap() {
            return;
        }
    }
    panic!("digest preparation must finish");
}

#[tokio::test]
async fn first_publication_queues_default_and_event_only_confirmed_subscribers() {
    let app = App::new().await;
    let default = subscriber(&app, "default@example.test", None, true).await;
    let event_only = subscriber(
        &app,
        "events@example.test",
        Some(
            json!({"all_notice_categories":false,"notice_category_ids":[],
            "uncategorized_notices":false,"documents":false,"events":true}),
        ),
        true,
    )
    .await;
    subscriber(
        &app,
        "documents@example.test",
        Some(
            json!({"all_notice_categories":false,"notice_category_ids":[],
            "uncategorized_notices":false,"documents":true,"events":false}),
        ),
        true,
    )
    .await;
    subscriber(&app, "pending@example.test", None, false).await;

    let event = create(&app, json!({"published":true})).await;
    assert_eq!(recipients(&app, event.id).await, vec![default, event_only]);
    assert!(announced_at(&app, event.id).await.is_some());
    let immediate_messages: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(
        immediate_messages, 0,
        "events must wait for the hourly digest"
    );
}

#[tokio::test]
async fn draft_publishes_once_and_edits_or_republication_do_not_add_late_subscribers() {
    let app = App::new().await;
    let original = subscriber(&app, "original@example.test", None, true).await;
    let draft = create(&app, json!({})).await;
    assert!(recipients(&app, draft.id).await.is_empty());
    assert_eq!(announced_at(&app, draft.id).await, None);

    let published = update(&app, &draft, json!({"published":true})).await;
    assert_eq!(recipients(&app, draft.id).await, vec![original]);
    let marker = announced_at(&app, draft.id).await;
    assert!(marker.is_some());
    prepare_all(&app, OffsetDateTime::now_utc() + Duration::hours(1)).await;
    assert!(recipients(&app, draft.id).await.is_empty());
    subscriber(&app, "late@example.test", None, true).await;

    let edited = update(
        &app,
        &published,
        json!({"title":"Upravený program setkání"}),
    )
    .await;
    let private = update(&app, &edited, json!({"published":false})).await;
    update(&app, &private, json!({"published":true})).await;
    assert_eq!(announced_at(&app, draft.id).await, marker);
    assert!(recipients(&app, draft.id).await.is_empty());
    let delivered_to: Vec<i64> = sqlx::query_scalar(
        "SELECT subscriber_id FROM mail_publication_items WHERE event_id=$1 ORDER BY subscriber_id",
    )
    .bind(draft.id)
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(delivered_to, vec![original]);
}

#[tokio::test]
async fn publication_without_recipients_still_excludes_future_subscribers() {
    let app = App::new().await;
    let published = create(&app, json!({"published":true})).await;
    assert!(announced_at(&app, published.id).await.is_some());
    subscriber(&app, "joined-later@example.test", None, true).await;
    let edited = update(&app, &published, json!({"location":"Kulturní dům"})).await;
    let private = update(&app, &edited, json!({"published":false})).await;
    update(&app, &private, json!({"published":true})).await;
    assert!(recipients(&app, published.id).await.is_empty());
}

#[tokio::test]
async fn cancelled_and_private_events_do_not_enter_digests() {
    let app = App::new().await;
    subscriber(&app, "reader@example.test", None, true).await;
    let cancelled = create(&app, json!({"published":true,"cancelled":true})).await;
    assert!(recipients(&app, cancelled.id).await.is_empty());
    assert_eq!(announced_at(&app, cancelled.id).await, None);
    let private = create(&app, json!({})).await;
    assert!(recipients(&app, private.id).await.is_empty());

    let later_cancelled = create(&app, json!({"published":true})).await;
    let later_private = create(&app, json!({"published":true})).await;
    update(&app, &later_cancelled, json!({"cancelled":true})).await;
    update(&app, &later_private, json!({"published":false})).await;
    prepare_all(&app, OffsetDateTime::now_utc() + Duration::hours(1)).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn historical_publication_marker_prevents_announcement_after_edit_or_republication() {
    let app = App::new().await;
    // Model an existing public row marked by the rollout migration, without
    // passing through the new publication hook or creating a historical queue.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO events(title,starts_at,ends_at,published,updated_at,email_announced_at)
         VALUES ('Původní akce','2030-07-10T10:00:00Z','2030-07-10T12:00:00Z',TRUE,
                 '2026-10-01T09:00:00Z',1700000000) RETURNING id",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    subscriber(&app, "after-rollout@example.test", None, true).await;
    let response = app
        .call("GET", &format!("/api/v1/admin/events/{id}"), None, true)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let old: Event = serde_json::from_value(body_json(response).await).unwrap();
    let edited = update(&app, &old, json!({"title":"Doplněná původní akce"})).await;
    let private = update(&app, &edited, json!({"published":false})).await;
    update(&app, &private, json!({"published":true})).await;
    assert_eq!(announced_at(&app, id).await, Some(1700000000));
    assert!(recipients(&app, id).await.is_empty());
}

#[tokio::test]
async fn failed_outbox_write_rolls_back_publication_and_announcement_marker() {
    let app = App::new().await;
    subscriber(&app, "atomic@example.test", None, true).await;
    let draft = create(&app, json!({})).await;
    sqlx::raw_sql(
        "CREATE FUNCTION reject_event_publication() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected outbox failure'; END; $$;
         CREATE TRIGGER reject_event_publication BEFORE INSERT ON publication_outbox
         FOR EACH ROW EXECUTE FUNCTION reject_event_publication();",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let response = app
        .call(
            "PUT",
            &format!("/api/v1/admin/events/{}", draft.id),
            Some(update_input(&draft, json!({"published":true}))),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let saved: (bool, Option<i64>, i64) =
        sqlx::query_as("SELECT published,email_announced_at,version FROM events WHERE id=$1")
            .bind(draft.id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(saved, (false, None, draft.version));
    assert!(recipients(&app, draft.id).await.is_empty());
}

struct Snapshot(Value);

impl OutageSource for Snapshot {
    async fn fetch(&self, town_code: &str) -> anyhow::Result<Vec<cez::Outage>> {
        cez::parse_snapshot(&serde_json::to_vec(&self.0)?, town_code)
    }
}

#[tokio::test]
async fn cez_creation_announces_once_while_repeat_edit_and_reappearance_do_not() {
    let mut app = App::new().await;
    let mut config = (*app.state.config).clone();
    config.cez_outages.enabled = true;
    config.cez_outages.interval_seconds = 14400;
    app.state = Backend::new(app.state.pool.clone(), config);
    app.router = backend::router(app.state.clone());
    let original = subscriber(&app, "outages@example.test", None, true).await;
    let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let mut value: Value = serde_json::from_str(include_str!("fixtures/cez-vysker.json")).unwrap();
    value["outages_in_town"][0]["opened_at"] =
        json!((now + Duration::days(10)).format(&Rfc3339).unwrap());
    value["outages_in_town"][0]["fix_expected_at"] = json!(
        (now + Duration::days(10) + Duration::hours(8))
            .format(&Rfc3339)
            .unwrap()
    );
    assert_eq!(
        sync_if_due(&app.state, &Snapshot(value.clone()), now)
            .await
            .unwrap()
            .unwrap()
            .created,
        1
    );
    let id: i64 =
        sqlx::query_scalar("SELECT id FROM events WHERE source_provider='cez_distribuce'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(recipients(&app, id).await, vec![original]);
    let marker = announced_at(&app, id).await;
    assert!(marker.is_some());
    prepare_all(&app, now + Duration::hours(1)).await;
    subscriber(&app, "new-outages@example.test", None, true).await;

    assert_eq!(
        sync_if_due(
            &app.state,
            &Snapshot(value.clone()),
            now + Duration::hours(4)
        )
        .await
        .unwrap()
        .unwrap()
        .updated,
        0
    );
    value["outages_in_town"][0]["fix_expected_at"] = json!(
        (now + Duration::days(10) + Duration::hours(9))
            .format(&Rfc3339)
            .unwrap()
    );
    assert_eq!(
        sync_if_due(
            &app.state,
            &Snapshot(value.clone()),
            now + Duration::hours(8)
        )
        .await
        .unwrap()
        .unwrap()
        .updated,
        1
    );
    for hour in [12, 16] {
        let report = sync_if_due(
            &app.state,
            &Snapshot(json!({"outages":null})),
            now + Duration::hours(hour),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(report.withdrawn, usize::from(hour == 16));
    }
    let restored = sync_if_due(&app.state, &Snapshot(value), now + Duration::hours(20))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.updated, 1);
    assert_eq!(restored.created, 0);
    assert_eq!(announced_at(&app, id).await, marker);
    assert!(recipients(&app, id).await.is_empty());
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mail_publication_items WHERE event_id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(queued, 1);
}
