#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::{App, body_json};
use http_body_util::BodyExt;
use obecni_web::backend::{
    self, Backend, cez,
    power_outages::{OutageSource, SyncReport, sync_if_due},
};
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

struct Source {
    response: Mutex<Option<Value>>,
    calls: AtomicUsize,
}

impl Source {
    fn new(value: Value) -> Self {
        Self {
            response: Mutex::new(Some(value)),
            calls: AtomicUsize::new(0),
        }
    }

    fn replace(&self, value: Value) {
        *self.response.lock().unwrap() = Some(value);
    }

    fn fail(&self) {
        *self.response.lock().unwrap() = None;
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl OutageSource for Source {
    async fn fetch(&self, town_code: &str) -> anyhow::Result<Vec<cez::Outage>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        let response = self.response.lock().unwrap().clone();
        let response = response.ok_or_else(|| anyhow::anyhow!("Upstream unavailable"))?;
        cez::parse_snapshot(&serde_json::to_vec(&response)?, town_code)
    }
}

async fn enabled_app() -> App {
    let mut app = App::new().await;
    let mut config = (*app.state.config).clone();
    config.cez_outages.enabled = true;
    app.state = Backend::new(app.state.pool.clone(), config);
    app.router = backend::router(app.state.clone());
    app
}

fn start() -> OffsetDateTime {
    OffsetDateTime::now_utc().replace_nanosecond(0).unwrap()
}

fn iso(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap()
}

fn snapshot(now: OffsetDateTime) -> Value {
    let mut value: Value = serde_json::from_str(include_str!("fixtures/cez-vysker.json")).unwrap();
    value["outages_in_town"][0]["opened_at"] = json!(iso(now + Duration::days(10)));
    value["outages_in_town"][0]["fix_expected_at"] =
        json!(iso(now + Duration::days(10) + Duration::hours(8)));
    value
}

fn empty_snapshot() -> Value {
    serde_json::from_str(include_str!("fixtures/cez-ktova-empty.json")).unwrap()
}

fn change_addresses(snapshot: &mut Value) {
    let town = snapshot["outages_in_town"][0]["addresses"]["towns"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|town| town["code"] == 577685)
        .unwrap();
    town["town_districts"][0]["town_parts"][0]["streets"][0]["house_nums"] = json!("9999");
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct ImportedEvent {
    id: i64,
    calendar_uid: String,
    version: i64,
    title: String,
    description: String,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    published: bool,
    cancelled: bool,
    updated_at: String,
}

async fn imported(app: &App) -> ImportedEvent {
    sqlx::query_as(
        "SELECT id,calendar_uid,version,title,description,starts_at,ends_at,published,cancelled,updated_at
         FROM events WHERE source_provider='cez_distribuce'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

async fn missing_since(app: &App) -> Option<OffsetDateTime> {
    sqlx::query_scalar("SELECT missing_since FROM cez_outage_imports")
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn feed(app: &App) -> String {
    let response = app.call("GET", "/api/v1/calendar.ics", None, false).await;
    assert_eq!(response.status(), StatusCode::OK);
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
    .replace("\r\n ", "")
}

async fn run(app: &App, source: &Source, now: OffsetDateTime) -> SyncReport {
    sync_if_due(&app.state, source, now)
        .await
        .unwrap()
        .expect("this poll is due")
}

#[tokio::test]
async fn repeated_snapshot_keeps_identity_revision_and_calendar_across_restart() {
    let app = enabled_app().await;
    let now = start();
    let source = Source::new(snapshot(now));
    assert_eq!(
        run(&app, &source, now).await,
        SyncReport {
            created: 1,
            ..Default::default()
        }
    );
    let original = imported(&app).await;
    assert!(original.published);
    assert!(!original.cancelled);
    assert!(original.description.contains("Drahoňovice"));
    assert!(original.description.contains("č. p. 1-11"));
    assert!(original.description.contains("parcelní čísla"));
    let calendar = feed(&app).await;
    assert!(calendar.contains(&format!("UID:{}", original.calendar_uid)));
    assert!(calendar.contains(&format!("SEQUENCE:{}", original.version)));
    assert!(calendar.contains("Drahoňovice"));
    assert!(calendar.contains("č. p. 1-11"));

    let restarted = Backend::new(app.state.pool.clone(), (*app.state.config).clone());
    assert!(
        sync_if_due(&restarted, &source, now + Duration::minutes(1))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(source.calls(), 1);
    assert_eq!(
        run(&app, &source, now + Duration::hours(1)).await,
        SyncReport::default()
    );
    assert_eq!(imported(&app).await, original);
    assert_eq!(feed(&app).await, calendar);
    assert_eq!(source.calls(), 2);
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE entity_type='event' AND operation LIKE 'cez_sync_%'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
}

#[tokio::test]
async fn date_address_and_explicit_cancellation_updates_keep_the_public_event() {
    let app = enabled_app().await;
    let now = start();
    let mut value = snapshot(now);
    let source = Source::new(value.clone());
    run(&app, &source, now).await;
    let original = imported(&app).await;

    change_addresses(&mut value);
    let changed_start = now + Duration::days(11);
    value["outages_in_town"][0]["opened_at"] = json!(iso(changed_start));
    value["outages_in_town"][0]["fix_expected_at"] = json!(iso(changed_start + Duration::hours(6)));
    source.replace(value.clone());
    assert_eq!(
        run(&app, &source, now + Duration::hours(1)).await.updated,
        1
    );
    let changed = imported(&app).await;
    assert_eq!(changed.id, original.id);
    assert_eq!(changed.calendar_uid, original.calendar_uid);
    assert_eq!(changed.version, original.version + 1);
    assert_eq!(changed.starts_at, changed_start);
    assert_eq!(changed.ends_at, changed_start + Duration::hours(6));
    assert!(changed.description.contains("č. p. 9999"));

    value["outages_in_town"][0]["cancelled_at"] = json!(iso(now + Duration::hours(2)));
    source.replace(value);
    assert_eq!(
        run(&app, &source, now + Duration::hours(2)).await.updated,
        1
    );
    let cancelled = imported(&app).await;
    assert_eq!(cancelled.id, original.id);
    assert!(cancelled.published);
    assert!(cancelled.cancelled);
    assert_eq!(cancelled.version, changed.version + 1);
    assert!(feed(&app).await.contains("STATUS:CANCELLED"));

    source.replace(empty_snapshot());
    for hours in [3, 4] {
        assert_eq!(
            run(&app, &source, now + Duration::hours(hours)).await,
            SyncReport::default()
        );
    }
    assert_eq!(imported(&app).await, cancelled);
    assert_eq!(missing_since(&app).await, None);
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/events/{}", original.id),
            None,
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn two_complete_missing_snapshots_withdraw_and_reappearance_resets_absence() {
    let app = enabled_app().await;
    let now = start();
    let value = snapshot(now);
    let source = Source::new(value.clone());
    run(&app, &source, now).await;
    let original = imported(&app).await;

    source.replace(empty_snapshot());
    assert_eq!(
        run(&app, &source, now + Duration::hours(1)).await.withdrawn,
        0
    );
    assert!(imported(&app).await.published);
    assert_eq!(missing_since(&app).await, Some(now + Duration::hours(1)));
    assert!(
        sync_if_due(&app.state, &source, now + Duration::minutes(90))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(source.calls(), 2);
    assert_eq!(
        run(&app, &source, now + Duration::hours(2)).await.withdrawn,
        1
    );
    let withdrawn = imported(&app).await;
    assert!(!withdrawn.published);
    assert!(!withdrawn.cancelled);
    assert_eq!(withdrawn.id, original.id);
    assert_eq!(withdrawn.version, original.version + 1);
    let withdrawn_calendar = feed(&app).await;
    assert!(withdrawn_calendar.contains(&format!("UID:{}", original.calendar_uid)));
    assert!(withdrawn_calendar.contains("STATUS:CANCELLED"));
    assert!(!withdrawn_calendar.contains("Drahoňovice"));

    source.replace(value);
    assert_eq!(
        run(&app, &source, now + Duration::hours(3)).await.updated,
        1
    );
    let restored = imported(&app).await;
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.calendar_uid, original.calendar_uid);
    assert!(restored.published);
    assert!(!restored.cancelled);
    assert_eq!(missing_since(&app).await, None);
    assert!(!feed(&app).await.contains("STATUS:CANCELLED"));

    source.replace(empty_snapshot());
    assert_eq!(
        run(&app, &source, now + Duration::hours(4)).await.withdrawn,
        0
    );
    assert!(imported(&app).await.published);
    assert_eq!(missing_since(&app).await, Some(now + Duration::hours(4)));
}

#[tokio::test]
async fn absence_after_start_keeps_ongoing_and_finished_event_published() {
    let app = enabled_app().await;
    let now = start();
    let source = Source::new(snapshot(now));
    run(&app, &source, now).await;
    let original = imported(&app).await;

    source.replace(empty_snapshot());
    // The fixture lasts eight hours. Exercise two missing snapshots while it
    // is ongoing, followed by two more after its scheduled end.
    for hours in [0, 1, 9, 10] {
        let checked_at = original.starts_at + Duration::hours(hours);
        assert_eq!(run(&app, &source, checked_at).await, SyncReport::default());
        assert_eq!(imported(&app).await, original);
        assert_eq!(missing_since(&app).await, None);
    }
    assert!(original.published);
    assert!(!original.cancelled);
}

#[tokio::test]
async fn invalid_partial_feed_transport_and_database_failures_preserve_all_saved_data() {
    let app = enabled_app().await;
    let now = start();
    let value = snapshot(now);
    let source = Source::new(value.clone());
    run(&app, &source, now).await;
    let original = imported(&app).await;
    let original_calendar = feed(&app).await;

    let mut partial = value.clone();
    change_addresses(&mut partial);
    let mut invalid = partial["outages_in_town"][0].clone();
    invalid["id"] = json!("110061127972");
    invalid.as_object_mut().unwrap().remove("addresses");
    partial["outages_in_town"]
        .as_array_mut()
        .unwrap()
        .push(invalid);
    source.replace(partial);
    assert!(
        sync_if_due(&app.state, &source, now + Duration::hours(1))
            .await
            .is_err()
    );
    assert_eq!(imported(&app).await, original);
    assert_eq!(missing_since(&app).await, None);
    assert_eq!(feed(&app).await, original_calendar);
    assert!(
        sync_if_due(&app.state, &source, now + Duration::minutes(90))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(source.calls(), 2);

    source.fail();
    assert!(
        sync_if_due(&app.state, &source, now + Duration::hours(2))
            .await
            .is_err()
    );
    assert_eq!(imported(&app).await, original);
    assert_eq!(missing_since(&app).await, None);

    // The existing event is updated first. Reject the new event's audit entry
    // after that update to verify that the complete reconciliation rolls back.
    let mut changed = value;
    change_addresses(&mut changed);
    let mut second = changed["outages_in_town"][0].clone();
    second["id"] = json!("110061127972");
    changed["outages_in_town"]
        .as_array_mut()
        .unwrap()
        .push(second);
    source.replace(changed);
    sqlx::raw_sql("CREATE FUNCTION reject_cez_create() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.operation='cez_sync_create' THEN RAISE EXCEPTION 'Synthetic import audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_cez_create BEFORE INSERT ON audit_log FOR EACH ROW EXECUTE FUNCTION reject_cez_create();")
        .execute(&app.state.pool).await.unwrap();
    assert!(
        sync_if_due(&app.state, &source, now + Duration::hours(3))
            .await
            .is_err()
    );
    assert_eq!(imported(&app).await, original);
    assert_eq!(feed(&app).await, original_calendar);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let status: (Option<OffsetDateTime>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT last_success_at,last_error,run_token FROM cez_outage_sync")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status.0, Some(now));
    assert!(status.1.is_some());
    assert!(status.2.is_none());
}

#[tokio::test]
async fn concurrent_pollers_share_a_single_persisted_claim() {
    let app = enabled_app().await;
    let now = start();
    let source = Source::new(snapshot(now));
    let replica = Backend::new(app.state.pool.clone(), (*app.state.config).clone());
    let (first, second) = tokio::join!(
        sync_if_due(&app.state, &source, now),
        sync_if_due(&replica, &source, now),
    );
    let reports = [first.unwrap(), second.unwrap()];
    assert_eq!(reports.iter().filter(|report| report.is_some()).count(), 1);
    assert_eq!(
        reports
            .iter()
            .flatten()
            .map(|report| report.created)
            .sum::<usize>(),
        1
    );
    assert_eq!(source.calls(), 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn disabled_import_does_not_contact_upstream_or_create_sync_state() {
    let app = App::new().await;
    let now = start();
    let source = Source::new(snapshot(now));
    assert!(
        sync_if_due(&app.state, &source, now)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(source.calls(), 0);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM cez_outage_sync")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn status_requires_authentication_and_reports_attempts_without_triggering_polling() {
    let app = enabled_app().await;
    let path = "/api/v1/admin/cez-outages/status";
    assert_eq!(
        app.call("GET", path, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let before = body_json(app.call("GET", path, None, true).await).await;
    assert_eq!(before["enabled"], true);
    assert_eq!(before["town_code"], "577685");
    assert_eq!(before["interval_seconds"], 3600);
    assert!(before["last_attempt_at"].is_null());
    assert!(before["last_success_at"].is_null());
    assert!(before["last_error"].is_null());

    let now = start();
    let source = Source::new(snapshot(now));
    run(&app, &source, now).await;
    source.fail();
    assert!(
        sync_if_due(&app.state, &source, now + Duration::hours(1))
            .await
            .is_err()
    );
    let after = body_json(app.call("GET", path, None, true).await).await;
    for (key, expected) in [
        ("last_success_at", now),
        ("last_attempt_at", now + Duration::hours(1)),
        ("next_attempt_at", now + Duration::hours(2)),
    ] {
        let parsed = OffsetDateTime::parse(after[key].as_str().unwrap(), &Rfc3339).unwrap();
        assert_eq!(parsed, expected);
    }
    assert_eq!(after["last_error"], "Upstream unavailable");
    assert_eq!(source.calls(), 2);
}
