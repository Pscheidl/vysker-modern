#![cfg(feature = "ssr")]
mod common;
#[path = "rental_v2_support/mod.rs"]
mod support;
use common::{App, body_json};
use obecni_web::backend::rentals;
use serde_json::json;
use support::*;
use time::{Date, Duration, Month, OffsetDateTime, Weekday};

// Search calendar dates independently of the recurrence generator under test.
fn matching_weekdays(from: Date, until: Date, weekday: Weekday, ordinal: u8) -> Vec<Date> {
    let mut date = from;
    let mut output = Vec::new();
    while date <= until {
        if date.weekday() == weekday && (date.day() - 1) / 7 + 1 == ordinal {
            output.push(date);
        }
        date += Duration::days(1);
    }
    output
}

fn future_year() -> i32 {
    obecni_web::backend::today(OffsetDateTime::now_utc()).year() + 2
}

fn monthly_input(asset: i64, first: Date, until: Date, hourly: bool) -> serde_json::Value {
    let mut input = admin_input(
        asset,
        first,
        hourly.then_some("20:00"),
        hourly.then_some("21:00"),
    );
    input["recurrence_interval_days"] = json!(0);
    input["recurrence_interval_months"] = json!(1);
    input["recurrence_until"] = json!(until.to_string());
    input
}

#[tokio::test]
async fn monthly_second_weekday_preserves_local_hours_across_year_and_dst_and_can_be_cancelled() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let year = future_year();
    let from = Date::from_calendar_date(year, Month::December, 1).unwrap();
    let until = Date::from_calendar_date(year + 1, Month::April, 30).unwrap();
    let expected = matching_weekdays(from, until, Weekday::Thursday, 2);
    assert_eq!(expected.len(), 5);
    let input = monthly_input(asset, expected[0], until, true);
    let response = app
        .call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true,
        )
        .await;
    assert_eq!(response.status(), 200);
    let preview = body_json(response).await;
    assert_eq!(preview["occurrence_count"], 5);
    assert_eq!(preview["conflict_count"], 0);
    assert_eq!(preview["total_price"], 125);
    for (occurrence, expected) in preview["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&expected)
    {
        assert_eq!(occurrence["start_date"], expected.to_string());
        assert_eq!(occurrence["end_date"], expected.to_string());
        assert_eq!(occurrence["start_time"], "20:00");
        assert_eq!(occurrence["end_time"], "21:00");
    }
    let created = create(&app, input).await;
    let series = created["series_id"].as_i64().unwrap();
    let records = created["rentals"].as_array().unwrap();
    assert_eq!(records.len(), 5);
    for (rental, expected) in records.iter().zip(&expected) {
        assert_eq!(rental["start_date"], expected.to_string());
        assert_eq!(rental["recurrence_interval_months"], 1);
        assert!(rental["recurrence_interval_days"].is_null());
        assert_eq!(rental["recurrence_until"], until.to_string());
    }
    let stored_monthly: bool = sqlx::query_scalar(
        "SELECT interval_days IS NULL AND interval_months=1 FROM rental_series WHERE id=$1",
    )
    .bind(series)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(stored_monthly);
    let utc_hours: Vec<f64> = sqlx::query_scalar("SELECT EXTRACT(HOUR FROM ((starts_local AT TIME ZONE 'Europe/Prague') AT TIME ZONE 'UTC'))::double precision FROM rentals WHERE series_id=$1 ORDER BY start_date").bind(series).fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(utc_hours, vec![19.0, 19.0, 19.0, 19.0, 18.0]);
    let first = records[0]["id"].as_i64().unwrap();
    let detail = body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{first}"), None, true)
            .await,
    )
    .await;
    assert_eq!(detail["recurrence_interval_months"], 1);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{first}/cancel"),
            None,
            true
        )
        .await
        .status(),
        200
    );
    let cancelled = body_json(
        app.call(
            "POST",
            &format!("/api/v1/admin/rental-series/{series}/cancel"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(cancelled["cancelled_count"], 4);
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rentals WHERE series_id=$1 AND status<>'cancelled'",
    )
    .bind(series)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(
        public(&app, asset, expected[0], Some("20:00"), Some("21:00"))
            .await
            .status(),
        201
    );
}

#[tokio::test]
async fn monthly_fifth_weekday_skips_missing_months_without_drifting_and_preserves_duration() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let from = Date::from_calendar_date(future_year(), Month::January, 1).unwrap();
    let first = matching_weekdays(from, from + Duration::days(150), Weekday::Thursday, 5)[0];
    let until = first + Duration::days(365);
    let expected = matching_weekdays(first, until, Weekday::Thursday, 5);
    assert!(expected.len() >= 4 && expected.len() < 12);
    assert!(
        expected
            .windows(2)
            .any(|pair| (pair[1] - pair[0]).whole_days() >= 56)
    );
    let mut input = monthly_input(asset, first, until, false);
    input["end_date"] = json!((first + Duration::days(2)).to_string());
    let response = app
        .call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true,
        )
        .await;
    assert_eq!(response.status(), 200);
    let preview = body_json(response).await;
    assert_eq!(
        preview["occurrence_count"].as_u64().unwrap() as usize,
        expected.len()
    );
    for (occurrence, expected) in preview["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&expected)
    {
        assert_eq!(occurrence["start_date"], expected.to_string());
        assert_eq!(
            occurrence["end_date"],
            (*expected + Duration::days(2)).to_string()
        );
        assert!(occurrence["start_time"].is_null());
    }
    let created = create(&app, input).await;
    assert_eq!(created["rentals"].as_array().unwrap().len(), expected.len());
    for (rental, expected) in created["rentals"].as_array().unwrap().iter().zip(&expected) {
        assert_eq!(rental["start_date"], expected.to_string());
        assert_eq!(
            rental["end_date"],
            (*expected + Duration::days(2)).to_string()
        );
    }
}

#[tokio::test]
async fn monthly_collision_preview_and_creation_never_leave_a_partial_series() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let from = Date::from_calendar_date(future_year(), Month::January, 1).unwrap();
    let until = Date::from_calendar_date(future_year(), Month::May, 31).unwrap();
    let expected = matching_weekdays(from, until, Weekday::Thursday, 2);
    let occupied = create(
        &app,
        admin_input(asset, expected[2], Some("20:00"), Some("21:00")),
    )
    .await;
    let occupied_id = occupied["rentals"][0]["id"].as_i64().unwrap();
    let input = monthly_input(asset, expected[0], until, true);
    let response = app
        .call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true,
        )
        .await;
    assert_eq!(response.status(), 200);
    let preview = body_json(response).await;
    assert_eq!(preview["occurrence_count"], 5);
    assert_eq!(preview["conflict_count"], 1);
    assert_eq!(preview["occurrences"][2]["conflict"], true);
    assert_eq!(
        app.call("POST", "/api/v1/admin/rentals", Some(input), true)
            .await
            .status(),
        409
    );
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM rentals")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(ids, vec![occupied_id]);
    let series: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_series")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(series, 0);
    let summaries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rental_mail_queue WHERE purpose='series_approval'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(summaries, 0);
}

#[tokio::test]
async fn invalid_monthly_rules_and_horizons_are_rejected_before_any_write() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let first = monday();
    let until = first + Duration::days(120);
    let valid = monthly_input(asset, first, until, false);
    let mut boundary = valid.clone();
    boundary["recurrence_until"] = json!((first + Duration::days(366)).to_string());
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(boundary),
            true
        )
        .await
        .status(),
        200
    );
    let mut invalid = Vec::new();
    for (days, months) in [(7, 1), (14, 1), (0, 2), (0, -1), (1, 1), (0, 0)] {
        let mut input = valid.clone();
        input["recurrence_interval_days"] = json!(days);
        input["recurrence_interval_months"] = json!(months);
        invalid.push(input);
    }
    let mut missing_until = valid.clone();
    missing_until["recurrence_until"] = serde_json::Value::Null;
    invalid.push(missing_until);
    let mut too_far = valid.clone();
    too_far["recurrence_until"] = json!((first + Duration::days(367)).to_string());
    invalid.push(too_far);
    let mut too_long = valid;
    too_long["end_date"] = json!((first + Duration::days(28)).to_string());
    invalid.push(too_long);
    for input in invalid {
        assert_eq!(
            app.call(
                "POST",
                "/api/v1/admin/rentals/preview",
                Some(input.clone()),
                true
            )
            .await
            .status(),
            400,
            "{input}"
        );
        assert_eq!(
            app.call("POST", "/api/v1/admin/rentals", Some(input.clone()), true)
                .await
                .status(),
            400,
            "{input}"
        );
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM rentals")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let series: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_series")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!((rows, series), (0, 0));
}

#[tokio::test]
async fn monthly_occurrences_without_energy_are_each_billed_and_closed_only_after_delivery() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    set_payment(&app).await;
    let from = Date::from_calendar_date(future_year(), Month::February, 1).unwrap();
    let until = Date::from_calendar_date(future_year(), Month::March, 31).unwrap();
    let expected = matching_weekdays(from, until, Weekday::Thursday, 2);
    let created = create(&app, monthly_input(asset, expected[0], until, true)).await;
    let ids: Vec<i64> = created["rentals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    sqlx::query("UPDATE rental_mail_queue SET cancelled_at=1,body=NULL WHERE purpose<>'invoice'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    for (index, id) in ids.iter().enumerate() {
        let end: OffsetDateTime = sqlx::query_scalar(
            "SELECT ends_local AT TIME ZONE 'Europe/Prague' FROM rentals WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        rentals::maintenance(&app.state, end - Duration::seconds(1))
            .await
            .unwrap();
        let before: i64 =
            sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE purpose='invoice'")
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
        assert_eq!(before, index as i64);
        rentals::maintenance(&app.state, end).await.unwrap();
        rentals::maintenance(&app.state, end).await.unwrap();
        let pending: (String, bool) =
            sqlx::query_as("SELECT status,require_meter FROM rentals WHERE id=$1")
                .bind(id)
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
        assert_eq!(pending, ("invoiced".into(), false));
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'",
        )
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        let (port, capture) = smtp_capture().await;
        let smtp =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
                .port(port)
                .build();
        assert!(rentals::deliver_one(&app.state, &smtp, end).await.unwrap());
        assert!(capture.await.unwrap().contains("vyuctovani-pronajmu.pdf"));
        let status: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(status, "closed");
        let retained: bool = sqlx::query_scalar("SELECT body IS NOT NULL OR invoice_attachment IS NOT NULL FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'").bind(id).fetch_one(&app.state.pool).await.unwrap();
        assert!(!retained);
        if index == 0 {
            let future: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
                .bind(ids[1])
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
            assert_eq!(future, "approved");
        }
    }
}

#[tokio::test]
async fn recurring_creation_is_private_atomic_and_cancellation_preserves_history() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let mut input = admin_input(asset, day, Some("20:00"), Some("21:00"));
    input["recurrence_interval_days"] = json!(7);
    input["recurrence_until"] = json!((day + Duration::days(21)).to_string());
    let occupied = create(
        &app,
        admin_input(
            asset,
            day + Duration::days(14),
            Some("20:00"),
            Some("21:00"),
        ),
    )
    .await;
    let blocked_id = occupied["rentals"][0]["id"].as_i64().unwrap();
    let preview = body_json(
        app.call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(preview["conflict_count"], 1);
    assert_eq!(preview["occurrence_count"], 4);
    assert_eq!(preview["occurrences"][2]["conflict"], true);
    assert_eq!(
        app.call("POST", "/api/v1/admin/rentals", Some(input.clone()), true)
            .await
            .status(),
        409
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rentals")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{blocked_id}/cancel"),
            None,
            true
        )
        .await
        .status(),
        200
    );
    let created = create(&app, input).await;
    let series = created["series_id"].as_i64().unwrap();
    assert_eq!(created["rentals"].as_array().unwrap().len(), 4);
    let summaries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rental_mail_queue WHERE purpose='series_approval'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(summaries, 1);
    assert_eq!(created["rentals"][0]["rental_price"], 25);
    assert_eq!(created["rentals"][0]["identity_uploaded"], false);
    assert_eq!(created["rentals"][0]["source"], "admin");
    let first = created["rentals"][0]["id"].as_i64().unwrap();
    sqlx::query("UPDATE rentals SET status='closed' WHERE id=$1")
        .bind(first)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        public(&app, asset, day, Some("20:00"), Some("21:00"))
            .await
            .status(),
        409
    );
    let cancelled = body_json(
        app.call(
            "POST",
            &format!("/api/v1/admin/rental-series/{series}/cancel"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(cancelled["cancelled_count"], 3);
    let cancellations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rental_mail_queue WHERE purpose='series_cancellation'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(cancellations, 1);
    let state: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(first)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "closed");
    assert_eq!(
        public(
            &app,
            asset,
            day + Duration::days(7),
            Some("20:00"),
            Some("21:00")
        )
        .await
        .status(),
        201
    );
    let available = body_json(
        app.call(
            "GET",
            &format!("/api/v1/rental-assets/{asset}/availability"),
            None,
            false,
        )
        .await,
    )
    .await
    .to_string();
    assert!(!available.contains("rental_price"));
    assert!(!available.contains("yoga"));
    assert!(!available.contains("Skupina"));
}

#[tokio::test]
async fn hourly_reminders_start_after_the_slot_and_replace_outdated_mail() {
    let app = App::new().await;
    let asset = asset(&app, true).await;
    let day = monday();
    let mut input = admin_input(asset, day, Some("20:00"), Some("21:00"));
    input["require_meter"] = json!(true);
    let created = create(&app, input).await;
    let id = created["rentals"][0]["id"].as_i64().unwrap();
    let end: OffsetDateTime = sqlx::query_scalar(
        "SELECT ends_local AT TIME ZONE 'Europe/Prague' FROM rentals WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    rentals::maintenance(&app.state, end - Duration::seconds(1))
        .await
        .unwrap();
    let early: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rental_mail_queue WHERE purpose IN ('reminder','reminder_admin')",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(early, 0);
    for days in 0..3 {
        rentals::maintenance(&app.state, end + Duration::days(days))
            .await
            .unwrap();
    }
    let current:i64=sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE purpose IN ('reminder','reminder_admin') AND cancelled_at IS NULL").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(current, 2);
    let old:i64=sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE purpose IN ('reminder','reminder_admin') AND cancelled_at IS NOT NULL AND body IS NULL").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(old, 4);
}

#[tokio::test]
async fn retried_series_confirmation_never_reconfirms_a_cancelled_occurrence() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let mut input = admin_input(asset, day, Some("20:00"), Some("21:00"));
    input["recurrence_interval_days"] = json!(7);
    input["recurrence_until"] = json!((day + Duration::days(7)).to_string());
    let created = create(&app, input).await;
    let id = created["rentals"][0]["id"].as_i64().unwrap();
    let failed =
        lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(9)
            .build();
    let now = OffsetDateTime::now_utc() + Duration::seconds(1);
    rentals::deliver_one(&app.state, &failed, now)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/cancel"),
            None,
            true
        )
        .await
        .status(),
        200
    );
    let preserved: bool = sqlx::query_scalar(
        "SELECT cancelled_at IS NULL FROM rental_mail_queue WHERE purpose='series_approval'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(preserved);
    let (port, capture) = smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    rentals::deliver_one(&app.state, &smtp, now + Duration::hours(1))
        .await
        .unwrap();
    let message = capture.await.unwrap();
    assert!(
        !message.contains(&day.to_string()),
        "cancelled date must not be confirmed"
    );
    assert!(message.contains(&(day + Duration::days(7)).to_string()));
}

#[tokio::test]
async fn automatic_hourly_billing_is_idempotent_and_closes_only_after_smtp_acceptance() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    set_payment(&app).await;
    let day = monday();
    let mut input = admin_input(asset, day, Some("20:00"), Some("21:00"));
    input["recurrence_interval_days"] = json!(14);
    input["recurrence_until"] = json!((day + Duration::days(14)).to_string());
    let created = create(&app, input).await;
    let id = created["rentals"][0]["id"].as_i64().unwrap();
    let other = created["rentals"][1]["id"].as_i64().unwrap();
    let at_end: OffsetDateTime = sqlx::query_scalar(
        "SELECT ends_local AT TIME ZONE 'Europe/Prague' FROM rentals WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    rentals::maintenance(&app.state, at_end - Duration::seconds(1))
        .await
        .unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE purpose='invoice'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(before, 0);
    rentals::maintenance(&app.state, at_end).await.unwrap();
    rentals::maintenance(&app.state, at_end).await.unwrap();
    let invoices: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE purpose='invoice'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(invoices, 1);
    let body: String = sqlx::query_scalar(
        "SELECT body FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(body.contains("20:00 do 21:00"));
    assert!(body.contains("Celkem k úhradě: 25 Kč"));
    assert!(body.contains("Číslo účtu: 19-2000145399/0800"));
    assert!(body.contains(&format!("Variabilní symbol: {id}")));
    let status: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "invoiced");
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/paid"),
            None,
            true
        )
        .await
        .status(),
        409
    );
    sqlx::query("UPDATE rental_mail_queue SET cancelled_at=1,body=NULL WHERE purpose<>'invoice'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let failed =
        lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(9)
            .build();
    rentals::deliver_one(&app.state, &failed, at_end)
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "invoiced");
    let (port, capture) = smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    rentals::deliver_one(&app.state, &smtp, at_end + Duration::hours(1))
        .await
        .unwrap();
    let mail = capture.await.unwrap();
    assert!(mail.contains("qr-platba.png"));
    assert!(mail.contains("vyuctovani-pronajmu.pdf"));
    let status: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "closed");
    let future: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(other)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(future, "approved");
    let retained:bool=sqlx::query_scalar("SELECT body IS NOT NULL OR invoice_attachment IS NOT NULL FROM rental_mail_queue WHERE rental_id=$1 AND purpose='invoice'").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert!(!retained);
}

#[tokio::test]
async fn automatic_billing_reports_missing_account_and_recovers_on_next_attempt() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let mut input = admin_input(asset, day, Some("20:00"), Some("21:00"));
    input["recurrence_interval_days"] = json!(7);
    input["recurrence_until"] = json!(day.to_string());
    input["wait_for_payment"] = json!(true);
    let created = create(&app, input).await;
    let id = created["rentals"][0]["id"].as_i64().unwrap();
    let now: OffsetDateTime = sqlx::query_scalar(
        "SELECT ends_local AT TIME ZONE 'Europe/Prague' FROM rentals WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    rentals::maintenance(&app.state, now).await.unwrap();
    let error: Option<String> = sqlx::query_scalar("SELECT billing_error FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(error.is_some());
    set_payment(&app).await;
    rentals::maintenance(&app.state, now + Duration::hours(1))
        .await
        .unwrap();
    let state: (String, Option<String>) =
        sqlx::query_as("SELECT status,billing_error FROM rentals WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(state, ("invoiced".into(), None));
}

#[tokio::test]
async fn adjacent_weeklong_occurrences_are_allowed_and_cover_every_hour_of_each_day() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let mut input = admin_input(asset, day, None, None);
    input["end_date"] = json!((day + Duration::days(6)).to_string());
    input["recurrence_interval_days"] = json!(7);
    input["recurrence_until"] = json!((day + Duration::days(14)).to_string());
    let preview_response = app
        .call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true,
        )
        .await;
    assert_eq!(preview_response.status(), 200);
    let preview = body_json(preview_response).await;
    assert_eq!(preview["occurrence_count"], 3);
    assert_eq!(preview["conflict_count"], 0);
    let result = create(&app, input).await;
    assert_eq!(result["rentals"].as_array().unwrap().len(), 3);
    assert_eq!(result["rentals"][0]["rental_price"], 25);
    assert!(result["rentals"][0]["start_time"].is_null());
    assert_eq!(
        public(&app, asset, day, Some("23:00"), Some("24:00"))
            .await
            .status(),
        409
    );
    assert_eq!(
        public(&app, asset, day + Duration::days(18), None, None)
            .await
            .status(),
        409
    );
    assert_eq!(
        public(
            &app,
            asset,
            day + Duration::days(21),
            Some("00:00"),
            Some("01:00")
        )
        .await
        .status(),
        201
    );
}

#[tokio::test]
async fn a_single_occupied_hour_prevents_a_multiday_public_or_admin_reservation() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let occupied_day = day + Duration::days(1);
    assert_eq!(
        public(&app, asset, occupied_day, Some("10:00"), Some("11:00"))
            .await
            .status(),
        201
    );
    let mut input = admin_input(asset, day, None, None);
    input["end_date"] = json!((day + Duration::days(2)).to_string());
    let response = app
        .call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true,
        )
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(body_json(response).await["conflict_count"], 1);
    assert_eq!(
        app.call("POST", "/api/v1/admin/rentals", Some(input), true)
            .await
            .status(),
        409
    );
    assert_eq!(
        public(&app, asset, occupied_day, None, None).await.status(),
        409
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rentals")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn overlapping_multiweek_period_cannot_create_a_partial_series() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let mut input = admin_input(asset, day, None, None);
    input["end_date"] = json!((day + Duration::days(7)).to_string());
    input["recurrence_interval_days"] = json!(7);
    input["recurrence_until"] = json!((day + Duration::days(14)).to_string());
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        app.call("POST", "/api/v1/admin/rentals", Some(input), true)
            .await
            .status(),
        400
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rentals")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let series: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_series")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(series, 0);
}

#[tokio::test]
async fn administrator_creation_preview_and_cancellation_require_rental_permission_and_csrf() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let input = admin_input(asset, day, Some("20:00"), Some("21:00"));
    let password = "Read-only-test-administrator-123!";
    obecni_web::backend::auth::create_admin(&app.state, "limited@example.test", password.into())
        .await
        .unwrap();
    let login = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"email":"limited@example.test","password":password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = body_json(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    for path in [
        "/api/v1/admin/rentals",
        "/api/v1/admin/rentals/preview",
        "/api/v1/admin/rentals/1/cancel",
        "/api/v1/admin/rental-series/1/cancel",
    ] {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(input.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{path}");
    }
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/rentals")
                .header("content-type", "application/json")
                .header("cookie", &app.cookie)
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}
