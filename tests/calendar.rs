#![cfg(feature = "ssr")]
mod common;

use axum::{http::StatusCode, response::Response};
use common::{App, body_json};
use http_body_util::BodyExt;
use serde_json::{Value, json};

fn input() -> Value {
    json!({"title":"Setkání sousedů","description":"**Program** pro občany.\nDruhý řádek.","location":"Náves","starts_at":"2030-07-10T12:00","ends_at":"2030-07-10T14:00","published":true,"cancelled":false,"category":"culture","all_day":false})
}

async fn create(app: &App, extra: Value) -> Value {
    let mut value = input();
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let response = app
        .call("POST", "/api/v1/admin/events", Some(value), true)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await
}

async fn update(app: &App, event: &Value, extra: Value) -> Value {
    let mut value = input();
    for key in [
        "title",
        "description",
        "location",
        "starts_at",
        "ends_at",
        "published",
        "cancelled",
        "category",
        "all_day",
        "repeat_every_14_days",
        "repeat_until",
    ] {
        value[key] = event[key].clone();
    }
    if event["all_day"] == true {
        for key in ["starts_at", "ends_at"] {
            value[key] = json!(&event[key].as_str().unwrap()[..10]);
        }
    }
    value["expected_version"] = event["version"].clone();
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let response = app
        .call(
            "PUT",
            &format!("/api/v1/admin/events/{}", event["id"]),
            Some(value),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn text(response: Response) -> String {
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

async fn feed(app: &App, category: &str) -> String {
    let response = app
        .call(
            "GET",
            &format!("/api/v1/calendar.ics{category}"),
            None,
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    text(response).await.replace("\r\n ", "")
}

fn property<'a>(ics: &'a str, key: &str) -> &'a str {
    ics.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}:")))
        .unwrap()
}

#[tokio::test]
async fn public_calendar_and_single_download_keep_identity_dates_text_and_private_boundary() {
    let app = App::new().await;
    let title = format!("{},;\\", "Žluťoučký kůň🐎".repeat(20));
    let event = create(&app, json!({"title":title.chars().take(190).collect::<String>(),"location":"Náves, Vyskeř; u školy\\dvůr"})).await;
    let private = create(&app, json!({"title":"Tajná schůzka","published":false})).await;
    let response = app.call("GET", "/api/v1/calendar.ics", None, false).await;
    assert_eq!(
        response.headers()["content-type"],
        "text/calendar; charset=utf-8"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let raw = text(response).await;
    assert!(raw.split("\r\n").all(|line| line.len() <= 75));
    assert!(!raw.replace("\r\n", "").contains(['\r', '\n']));
    let calendar = raw.replace("\r\n ", "");
    assert_eq!(calendar.matches("BEGIN:VEVENT").count(), 1);
    assert!(calendar.contains("DTSTART:20300710T100000Z\r\n"));
    assert!(calendar.contains("DTEND:20300710T120000Z\r\n"));
    assert!(calendar.contains("LOCATION:Náves\\, Vyskeř\\; u školy\\\\dvůr"));
    assert!(calendar.contains("DESCRIPTION:Program pro občany.\\nDruhý řádek."));
    assert!(calendar.contains(&format!(
        "URL:http://localhost:3000/kalendar/{}",
        event["id"]
    )));
    assert!(!calendar.contains("Tajná"));
    let download = app
        .call(
            "GET",
            &format!("/api/v1/events/{}/calendar.ics", event["id"]),
            None,
            false,
        )
        .await;
    assert_eq!(
        download.headers()["content-disposition"],
        "inline; filename=\"akce.ics\""
    );
    let single = text(download).await.replace("\r\n ", "");
    assert_eq!(property(&single, "UID"), property(&calendar, "UID"));
    assert_eq!(single, calendar);
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/events/{}/calendar.ics", private["id"]),
            None,
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call("GET", "/api/v1/events/999999/calendar.ics", None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        feed(&app, "?category=waste")
            .await
            .matches("BEGIN:VEVENT")
            .count(),
        0
    );
    assert_eq!(
        feed(&app, "?category=culture")
            .await
            .matches("BEGIN:VEVENT")
            .count(),
        1
    );
    assert_eq!(
        app.call("GET", "/api/v1/calendar.ics?category=secret", None, false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call("GET", "/api/v1/calendar.ics?published=false", None, false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn subscriptions_receive_changes_cancellations_moves_withdrawal_and_republication() {
    let app = App::new().await;
    let event = create(&app, json!({})).await;
    let first = feed(&app, "").await;
    assert_eq!(
        feed(&app, "").await,
        first,
        "unchanged fetch is byte stable"
    );
    let uid = property(&first, "UID").to_owned();
    let event = update(
        &app,
        &event,
        json!({"starts_at":"2030-07-11T15:00","ends_at":"2030-07-11T17:00","cancelled":true}),
    )
    .await;
    let cancelled = feed(&app, "").await;
    assert_eq!(property(&cancelled, "UID"), uid);
    assert_eq!(property(&cancelled, "SEQUENCE"), "2");
    assert!(cancelled.contains("STATUS:CANCELLED"));
    assert!(cancelled.contains("DTSTART:20300711T130000Z"));
    assert_eq!(cancelled.matches("BEGIN:VEVENT").count(), 1);
    let event = update(
        &app,
        &event,
        json!({"category":"council","cancelled":false}),
    )
    .await;
    let old_category = feed(&app, "?category=culture").await;
    assert_eq!(property(&old_category, "UID"), uid);
    assert_eq!(property(&old_category, "SEQUENCE"), "3");
    assert!(old_category.contains("STATUS:CANCELLED"));
    assert!(!old_category.contains("Setkání sousedů"));
    let whole = feed(&app, "").await;
    assert!(whole.contains("STATUS:CONFIRMED"));
    assert_eq!(whole.matches("BEGIN:VEVENT").count(), 1);
    assert_eq!(
        feed(&app, "?category=council")
            .await
            .matches("BEGIN:VEVENT")
            .count(),
        1
    );
    let event = update(&app, &event, json!({"published":false,"title":"Nově soukromý název","location":"Soukromé místo","description":"Tajný text"})).await;
    let withdrawn = feed(&app, "").await;
    assert!(withdrawn.contains("STATUS:CANCELLED"));
    assert_eq!(property(&withdrawn, "UID"), uid);
    assert_eq!(property(&withdrawn, "SEQUENCE"), "4");
    for secret in [
        "Setkání",
        "Soukromé",
        "Tajný",
        "Nově",
        "LOCATION:",
        "DESCRIPTION:",
        "URL:",
    ] {
        assert!(!withdrawn.contains(secret));
    }
    let event = update(
        &app,
        &event,
        json!({"title":"Znovu zveřejněno","published":true}),
    )
    .await;
    let restored = feed(&app, "").await;
    assert_eq!(property(&restored, "UID"), uid);
    assert_eq!(property(&restored, "SEQUENCE"), "5");
    assert_eq!(restored.matches("BEGIN:VEVENT").count(), 1);
    assert!(restored.contains("STATUS:CONFIRMED"));
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/admin/events/{}", event["id"]),
            Some(json!({"expected_version":event["version"]})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let deleted = feed(&app, "").await;
    assert_eq!(property(&deleted, "UID"), uid);
    assert_eq!(property(&deleted, "SEQUENCE"), "6");
    assert!(deleted.contains("STATUS:CANCELLED"));
    assert!(!deleted.contains("Znovu zveřejněno"));
}

#[tokio::test]
async fn all_day_dates_span_dst_and_archive_without_24_hour_assumptions() {
    let app = App::new().await;
    let event = create(
        &app,
        json!({"all_day":true,"category":"waste","starts_at":"2026-03-28","ends_at":"2026-03-30"}),
    )
    .await;
    assert_eq!(event["starts_at"], "2026-03-28T00:00:00+01:00");
    assert_eq!(event["ends_at"], "2026-03-30T23:59:59+02:00");
    let calendar = feed(&app, "").await;
    assert!(calendar.contains("DTSTART;VALUE=DATE:20260328\r\nDTEND;VALUE=DATE:20260331"));
    assert!(!calendar.contains("původní"));
    let event = update(
        &app,
        &event,
        json!({"starts_at":"2026-10-25","ends_at":"2026-10-25"}),
    )
    .await;
    assert_eq!(event["starts_at"], "2026-10-25T00:00:00+02:00");
    assert_eq!(event["ends_at"], "2026-10-25T23:59:59+01:00");
    assert!(
        feed(&app, "")
            .await
            .contains("DTSTART;VALUE=DATE:20261025\r\nDTEND;VALUE=DATE:20261026")
    );
    for extra in [
        json!({"all_day":true,"starts_at":"2026-02-30","ends_at":"2026-03-01"}),
        json!({"all_day":true,"starts_at":"2026-03-30","ends_at":"2026-03-28"}),
        json!({"all_day":true,"starts_at":"9999-12-31","ends_at":"9999-12-31"}),
        json!({"category":"unknown"}),
    ] {
        let mut value = input();
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            app.call("POST", "/api/v1/admin/events", Some(value), true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn import_mutations_get_revisions_and_imprecise_times_do_not_invent_clock_times() {
    let app = App::new().await;
    let event = create(&app, json!({})).await;
    sqlx::query("UPDATE events SET start_time_known=FALSE,end_time_known=FALSE,end_date_known=FALSE,description='Importovaný text' WHERE id=$1")
        .bind(event["id"].as_i64().unwrap()).execute(&app.state.pool).await.unwrap();
    let calendar = feed(&app, "").await;
    assert_eq!(property(&calendar, "SEQUENCE"), "2");
    assert!(calendar.contains("DTSTART;VALUE=DATE:20300710\r\nDTEND;VALUE=DATE:20300711"));
    assert!(calendar.contains("Původní web čas začátku neuváděl"));
    assert!(!calendar.contains("T100000Z"));
    sqlx::query("UPDATE events SET description='Aktualizováno importem' WHERE id=$1")
        .bind(event["id"].as_i64().unwrap())
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(property(&feed(&app, "").await, "SEQUENCE"), "3");
    sqlx::query("DELETE FROM events WHERE id=$1")
        .bind(event["id"].as_i64().unwrap())
        .execute(&app.state.pool)
        .await
        .unwrap();
    let deleted = feed(&app, "").await;
    assert_eq!(property(&deleted, "SEQUENCE"), "4");
    assert!(deleted.contains("STATUS:CANCELLED"));
}

#[tokio::test]
async fn subsecond_end_does_not_emit_equal_ics_start_and_end() {
    let app = App::new().await;
    let event = create(
        &app,
        json!({"starts_at":"2030-07-10T12:00:00.100000","ends_at":"2030-07-10T12:00:00.200000"}),
    )
    .await;
    let calendar = feed(&app, "").await;
    assert!(calendar.contains("DTSTART:20300710T100000Z"));
    assert!(!calendar.contains("DTEND:"));
    update(
        &app,
        &event,
        json!({"ends_at":"2030-07-10T12:00:01.100000"}),
    )
    .await;
    assert!(feed(&app, "").await.contains("DTEND:20300710T100001Z"));
}

#[tokio::test]
async fn fortnightly_all_day_series_keeps_identity_and_previous_rule_on_withdrawal() {
    let app = App::new().await;
    let event = create(
        &app,
        json!({
            "title":"Svoz odpadu", "category":"waste", "all_day":true,
            "starts_at":"2026-03-20", "ends_at":"2026-03-21",
            "repeat_every_14_days":true, "repeat_until":"2026-04-17"
        }),
    )
    .await;
    let calendar = feed(&app, "").await;
    let uid = property(&calendar, "UID").to_owned();
    assert_eq!(calendar.matches("BEGIN:VEVENT").count(), 1);
    assert!(!calendar.contains("VTIMEZONE"));
    assert!(calendar.contains("DTSTART;VALUE=DATE:20260320\r\nDTEND;VALUE=DATE:20260322"));
    assert_eq!(
        property(&calendar, "RRULE"),
        "FREQ=WEEKLY;INTERVAL=2;UNTIL=20260417"
    );
    let single = text(
        app.call(
            "GET",
            &format!("/api/v1/events/{}/calendar.ics", event["id"]),
            None,
            false,
        )
        .await,
    )
    .await
    .replace("\r\n ", "");
    assert_eq!(single, calendar);

    let cancelled = update(&app, &event, json!({"cancelled":true})).await;
    let calendar = feed(&app, "").await;
    assert_eq!(property(&calendar, "UID"), uid);
    assert_eq!(property(&calendar, "SEQUENCE"), "2");
    assert_eq!(
        property(&calendar, "RRULE"),
        "FREQ=WEEKLY;INTERVAL=2;UNTIL=20260417"
    );
    assert_eq!(property(&calendar, "STATUS"), "CANCELLED");

    let moved = update(
        &app,
        &cancelled,
        json!({"category":"council", "cancelled":false, "repeat_until":"2026-05-01"}),
    )
    .await;
    let old_category = feed(&app, "?category=waste").await;
    assert_eq!(property(&old_category, "UID"), uid);
    assert_eq!(property(&old_category, "SEQUENCE"), "3");
    assert_eq!(property(&old_category, "STATUS"), "CANCELLED");
    assert_eq!(
        property(&old_category, "RRULE"),
        "FREQ=WEEKLY;INTERVAL=2;UNTIL=20260417"
    );
    assert!(!old_category.contains("Svoz odpadu"));

    update(
        &app,
        &moved,
        json!({"published":false, "title":"Soukromý plán", "repeat_every_14_days":false, "repeat_until":null}),
    )
    .await;
    for category in ["", "?category=council"] {
        let withdrawn = feed(&app, category).await;
        assert_eq!(withdrawn.matches("BEGIN:VEVENT").count(), 1);
        assert_eq!(property(&withdrawn, "UID"), uid);
        assert_eq!(property(&withdrawn, "SEQUENCE"), "4");
        assert_eq!(property(&withdrawn, "STATUS"), "CANCELLED");
        assert_eq!(
            property(&withdrawn, "RRULE"),
            "FREQ=WEEKLY;INTERVAL=2;UNTIL=20260501"
        );
        assert!(!withdrawn.contains("Soukromý plán"));
    }
}

#[tokio::test]
async fn fortnightly_timed_series_uses_prague_timezone_and_utc_inclusive_until() {
    let app = App::new().await;
    let event = create(
        &app,
        json!({
            "starts_at":"2026-03-15T09:00", "ends_at":"2026-03-15T11:00",
            "repeat_every_14_days":true, "repeat_until":"2026-04-12"
        }),
    )
    .await;
    let calendar = feed(&app, "").await;
    let uid = property(&calendar, "UID").to_owned();
    assert_eq!(calendar.matches("BEGIN:VEVENT").count(), 1);
    assert_eq!(calendar.matches("BEGIN:VTIMEZONE").count(), 1);
    assert!(calendar.contains("TZID:Europe/Prague"));
    assert!(calendar.contains("DTSTART:19960101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100"));
    assert!(calendar.contains("DTSTART:19960331T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200"));
    assert!(calendar.contains("DTSTART:19961027T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100"));
    assert!(calendar.contains("RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU"));
    assert!(calendar.contains("RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU"));
    let component = calendar.split("BEGIN:VEVENT\r\n").nth(1).unwrap();
    assert_eq!(
        property(component, "DTSTART;TZID=Europe/Prague"),
        "20260315T090000"
    );
    assert_eq!(property(component, "DURATION"), "PT7200S");
    assert_eq!(
        property(component, "RRULE"),
        "FREQ=WEEKLY;INTERVAL=2;UNTIL=20260412T215959Z"
    );
    assert!(!component.contains("DTEND"));
    let single = text(
        app.call(
            "GET",
            &format!("/api/v1/events/{}/calendar.ics", event["id"]),
            None,
            false,
        )
        .await,
    )
    .await
    .replace("\r\n ", "");
    assert_eq!(single, calendar);

    let autumn = update(
        &app,
        &event,
        json!({"starts_at":"2026-10-11T09:00", "ends_at":"2026-10-11T11:00", "repeat_until":"2026-11-08"}),
    )
    .await;
    let calendar = feed(&app, "").await;
    assert!(calendar.contains("DTSTART;TZID=Europe/Prague:20261011T090000"));
    assert!(calendar.contains("RRULE:FREQ=WEEKLY;INTERVAL=2;UNTIL=20261108T225959Z"));
    assert_eq!(property(&calendar, "UID"), uid);
    assert_eq!(property(&calendar, "SEQUENCE"), "2");

    let unbounded = update(&app, &autumn, json!({"repeat_until":null})).await;
    let calendar = feed(&app, "").await;
    assert!(calendar.contains("RRULE:FREQ=WEEKLY;INTERVAL=2\r\n"));
    assert!(!calendar.contains("UNTIL="));
    assert_eq!(property(&calendar, "UID"), uid);
    assert_eq!(property(&calendar, "SEQUENCE"), "3");
    sqlx::query("DELETE FROM events WHERE id=$1")
        .bind(unbounded["id"].as_i64().unwrap())
        .execute(&app.state.pool)
        .await
        .unwrap();
    let deleted = feed(&app, "").await;
    assert_eq!(property(&deleted, "UID"), uid);
    assert_eq!(property(&deleted, "SEQUENCE"), "4");
    assert_eq!(property(&deleted, "STATUS"), "CANCELLED");
    assert!(deleted.contains("RRULE:FREQ=WEEKLY;INTERVAL=2\r\n"));
    assert!(deleted.contains("DTSTART;TZID=Europe/Prague:20261011T090000"));
    assert!(deleted.contains("DURATION:PT7200S"));
    assert!(deleted.contains("BEGIN:VTIMEZONE"));
}

#[tokio::test]
async fn recurring_timed_duration_keeps_elapsed_seconds_across_clock_change() {
    let app = App::new().await;
    create(
        &app,
        json!({
            "starts_at":"2026-03-28T12:00", "ends_at":"2026-03-29T12:00",
            "repeat_every_14_days":true
        }),
    )
    .await;
    let calendar = feed(&app, "").await;
    assert!(calendar.contains("DTSTART;TZID=Europe/Prague:20260328T120000"));
    assert!(calendar.contains("DURATION:PT82800S"));
}

#[tokio::test]
async fn outage_in_general_calendar_includes_addresses_and_keeps_identity_when_cancelled() {
    let app = App::new().await;
    let event = create(
        &app,
        json!({
            "title":"Odstávka elektřiny",
            "description":"Dotčené adresy:\nVyskeř č. p. 10\nVyskeř č. p. 11",
            "location":"Vyskeř",
            "category":"other"
        }),
    )
    .await;
    sqlx::query("UPDATE events SET source_provider='cez_distribuce',source_id='outage-456',source_url='https://www.cezdistribuce.cz/odstavky' WHERE id=$1")
        .bind(event["id"].as_i64().unwrap()).execute(&app.state.pool).await.unwrap();
    let calendar = feed(&app, "").await;
    assert!(calendar.contains("SUMMARY:Odstávka elektřiny"));
    assert!(calendar.contains("Vyskeř č. p. 10\\nVyskeř č. p. 11"));
    assert!(calendar.contains("CATEGORIES:Ostatní"));
    assert!(
        !feed(&app, "?category=culture")
            .await
            .contains("BEGIN:VEVENT")
    );
    let uid = property(&calendar, "UID").to_owned();
    let version = property(&calendar, "SEQUENCE").parse::<i64>().unwrap();

    sqlx::query("UPDATE events SET cancelled=TRUE WHERE id=$1")
        .bind(event["id"].as_i64().unwrap())
        .execute(&app.state.pool)
        .await
        .unwrap();
    let cancelled = feed(&app, "").await;
    assert_eq!(property(&cancelled, "UID"), uid);
    assert_eq!(property(&cancelled, "STATUS"), "CANCELLED");
    assert_eq!(
        property(&cancelled, "SEQUENCE").parse::<i64>().unwrap(),
        version + 1
    );

    sqlx::query("DELETE FROM events WHERE id=$1")
        .bind(event["id"].as_i64().unwrap())
        .execute(&app.state.pool)
        .await
        .unwrap();
    let withdrawn = feed(&app, "").await;
    assert_eq!(property(&withdrawn, "UID"), uid);
    assert_eq!(property(&withdrawn, "STATUS"), "CANCELLED");
    assert!(!withdrawn.contains("Vyskeř č. p. 10"));
}
