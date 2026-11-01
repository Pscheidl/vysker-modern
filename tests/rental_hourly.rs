#![cfg(feature = "ssr")]
mod common;
#[path = "rental_v2_support/mod.rs"]
mod support;
use common::{App, body_json};
use obecni_web::backend::today;
use serde_json::json;
use support::*;
use time::{Date, Duration, Month, OffsetDateTime, Weekday};
use time_tz::{OffsetDateTimeExt, PrimitiveDateTimeExt, timezones::db::europe::PRAGUE};

#[tokio::test]
async fn every_booking_endpoint_rejects_past_hours_and_started_whole_days() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let now = OffsetDateTime::now_utc();
    let day = today(now);
    let mut past_weekday = day;
    while matches!(past_weekday.weekday(), Weekday::Saturday | Weekday::Sunday) {
        past_weekday -= Duration::days(1);
    }
    let hour = if past_weekday == day {
        now.to_timezone(PRAGUE).hour()
    } else {
        23
    };
    let from = format!("{hour:02}:00");
    let until = format!("{:02}:00", hour + 1);
    for (date, from, until) in [
        (day - Duration::days(1), None, None),
        (day, None, None),
        (past_weekday, Some(from.as_str()), Some(until.as_str())),
    ] {
        let response = public(&app, asset, date, from, until).await;
        assert_eq!(response.status(), 400);
        assert!(
            body_json(response).await["error"]
                .as_str()
                .unwrap()
                .contains("budoucnosti")
        );
        let input = admin_input(asset, date, from, until);
        for path in ["/api/v1/admin/rentals/preview", "/api/v1/admin/rentals"] {
            let response = app.call("POST", path, Some(input.clone()), true).await;
            assert_eq!(response.status(), 400, "{path} accepted {date} {from:?}");
            assert!(
                body_json(response).await["error"]
                    .as_str()
                    .unwrap()
                    .contains("budoucnosti")
            );
        }
        let multipart = admin_upload(&app, input, false).await;
        assert_eq!(multipart.status(), 400);
        assert!(
            body_json(multipart).await["error"]
                .as_str()
                .unwrap()
                .contains("budoucnosti")
        );
    }
    // A future repeat date does not make an already-started first occurrence valid.
    for (days, months) in [(7, 0), (14, 0), (0, 1)] {
        let mut input = admin_input(asset, day, None, None);
        input["recurrence_interval_days"] = json!(days);
        input["recurrence_interval_months"] = json!(months);
        input["recurrence_until"] = json!((day + Duration::days(60)).to_string());
        for path in ["/api/v1/admin/rentals/preview", "/api/v1/admin/rentals"] {
            assert_eq!(
                app.call("POST", path, Some(input.clone()), true)
                    .await
                    .status(),
                400
            );
        }
    }
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM rentals),(SELECT count(*) FROM rental_series),(SELECT count(*) FROM rental_mail_queue)")
        .fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(counts, (0, 0, 0));

    let future = monday();
    let input = admin_input(asset, future, None, None);
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/admin/rentals/preview",
            Some(input.clone()),
            true
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        app.call("POST", "/api/v1/admin/rentals", Some(input), true)
            .await
            .status(),
        201
    );
    assert_eq!(
        public(
            &app,
            asset,
            future + Duration::days(1),
            Some("10:00"),
            Some("11:00")
        )
        .await
        .status(),
        201
    );
}

#[tokio::test]
async fn invoice_available_timestamp_is_the_exclusive_prague_end_of_the_rental() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let future_year = today(OffsetDateTime::now_utc()).year() + 1;
    let final_sunday = |month| {
        let mut day = Date::from_calendar_date(future_year, month, 31).unwrap();
        while day.weekday() != Weekday::Sunday {
            day -= Duration::days(1)
        }
        day
    };
    let cases = [
        (
            day,
            Some("18:00"),
            Some("19:00"),
            day.with_hms(19, 0, 0).unwrap(),
        ),
        (
            day + Duration::days(1),
            Some("23:00"),
            Some("24:00"),
            (day + Duration::days(2)).midnight(),
        ),
        (
            day + Duration::days(3),
            None,
            None,
            (day + Duration::days(4)).midnight(),
        ),
        (
            final_sunday(Month::March),
            None,
            None,
            (final_sunday(Month::March) + Duration::days(1)).midnight(),
        ),
        (
            final_sunday(Month::October),
            None,
            None,
            (final_sunday(Month::October) + Duration::days(1)).midnight(),
        ),
    ];
    for (date, from, to, end) in cases {
        let created = create(&app, admin_input(asset, date, from, to)).await;
        let rental = &created["rentals"][0];
        let expected = format!("{}T{:02}:{:02}:00", end.date(), end.hour(), end.minute());
        let expected_unix = end.assume_timezone(PRAGUE).unwrap().unix_timestamp();
        assert_eq!(rental["invoice_available_at"], expected);
        assert_eq!(rental["invoice_available_unix"], expected_unix);
        let id = rental["id"].as_i64().unwrap();
        let detail = body_json(
            app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
                .await,
        )
        .await;
        assert_eq!(detail["invoice_available_at"], expected);
        assert_eq!(detail["invoice_available_unix"], expected_unix);
    }
    let list = body_json(app.call("GET", "/api/v1/admin/rentals", None, true).await).await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .all(|rental| rental["invoice_available_at"].as_str().is_some()
                && rental["invoice_available_unix"].as_i64().is_some())
    );
}

#[tokio::test]
async fn hourly_conflicts_allow_adjacent_hours_but_never_a_whole_occupied_day() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let first = public(&app, asset, day, Some("18:00"), Some("19:00")).await;
    assert_eq!(first.status(), 201);
    let id = body_json(first).await["id"].as_i64().unwrap();
    // Even if both ends are free, a continuous range cannot skip the occupied middle hour.
    assert_eq!(
        public(&app, asset, day, Some("17:00"), Some("20:00"))
            .await
            .status(),
        409
    );
    assert_eq!(
        public(&app, asset, day, Some("19:00"), Some("20:00"))
            .await
            .status(),
        201
    );
    assert_eq!(
        public(&app, asset, day, Some("18:00"), Some("20:00"))
            .await
            .status(),
        409
    );
    assert_eq!(public(&app, asset, day, None, None).await.status(), 409);
    assert_eq!(
        public(&app, asset, day, Some("00:00"), Some("24:00"))
            .await
            .status(),
        409
    );
    let detail = body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
            .await,
    )
    .await;
    assert_eq!(detail["rental_price"], 80);
    assert_eq!(detail["start_time"], "18:00");
    assert_eq!(detail["source"], "public");
    assert_eq!(detail["identity_uploaded"], true);
    let availability = body_json(
        app.call(
            "GET",
            &format!("/api/v1/rental-assets/{asset}/availability"),
            None,
            false,
        )
        .await,
    )
    .await;
    let text = availability.to_string();
    assert!(!text.contains("rental_price"));
    assert!(!text.contains("Jan"));
    assert!(text.contains("18:00"));
    sqlx::query("UPDATE rentals SET status='paid' WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        public(&app, asset, day, Some("18:00"), Some("19:00"))
            .await
            .status(),
        409
    );
    assert_eq!(app.call("POST",&format!("/api/v1/admin/rental-assets/{asset}/blocks"),Some(json!({"start_date":day.to_string(),"end_date":day.to_string(),"start_time":"20:00","end_time":"21:00"})),true).await.status(),201);
    assert_eq!(
        public(&app, asset, day, Some("20:00"), Some("21:00"))
            .await
            .status(),
        409
    );
}

#[tokio::test]
async fn friday_supports_hours_or_a_whole_day_and_weekends_only_whole_days() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let friday = monday() + Duration::days(4);
    let response = public(&app, asset, friday, Some("18:00"), Some("20:00")).await;
    assert_eq!(response.status(), 201);
    let id = body_json(response).await["id"].as_i64().unwrap();
    let price: i64 = sqlx::query_scalar("SELECT rental_price FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(price, 160);
    let saturday = friday + Duration::days(1);
    let sunday = friday + Duration::days(2);
    assert_eq!(
        public(&app, asset, saturday, Some("18:00"), Some("19:00"))
            .await
            .status(),
        400
    );
    assert_eq!(
        public(&app, asset, saturday, None, None).await.status(),
        201
    );
    assert_eq!(public(&app, asset, sunday, None, None).await.status(), 201);
    let rows: Vec<i64> = sqlx::query_scalar(
        "SELECT rental_price FROM rentals WHERE asset_id=$1 AND start_date>$2 ORDER BY start_date",
    )
    .bind(asset)
    .bind(friday)
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![700, 700]);
    let following_friday = friday + Duration::days(7);
    let whole = public(&app, asset, following_friday, None, None).await;
    assert_eq!(whole.status(), 201);
    let whole_id = body_json(whole).await["id"].as_i64().unwrap();
    let whole_price: i64 = sqlx::query_scalar("SELECT rental_price FROM rentals WHERE id=$1")
        .bind(whole_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(whole_price, 700);
    assert_eq!(
        public(&app, asset, following_friday, Some("18:00"), Some("19:00"))
            .await
            .status(),
        409
    );
    assert_eq!(
        public(&app, asset, monday(), Some("18:30"), Some("19:30"))
            .await
            .status(),
        400
    );
    assert_eq!(
        public(&app, asset, monday(), Some("19:00"), Some("18:00"))
            .await
            .status(),
        400
    );
}

#[tokio::test]
async fn concurrent_hour_and_day_requests_cannot_both_reserve_the_same_time() {
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let (hour, whole) = tokio::join!(
        public(&app, asset, day, Some("23:00"), Some("24:00")),
        public(&app, asset, day, None, None)
    );
    let mut statuses = vec![hour.status().as_u16(), whole.status().as_u16()];
    statuses.sort_unstable();
    assert_eq!(statuses, vec![201, 409]);
}

#[tokio::test]
async fn approval_reloads_the_period_after_waiting_for_the_rental_lock() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let app = App::new().await;
    let asset = asset(&app, false).await;
    let day = monday();
    let response = public(&app, asset, day, Some("20:00"), Some("21:00")).await;
    assert_eq!(response.status(), 201);
    let id = body_json(response).await["id"].as_i64().unwrap();
    let mut locked = app.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM rentals WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *locked)
        .await
        .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/rentals/{id}/approve"))
        .header("cookie", &app.cookie)
        .header("x-csrf-token", &app.csrf)
        .body(Body::empty())
        .unwrap();
    let router = app.router.clone();
    let approval = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    sqlx::query("UPDATE rentals SET start_date=$2,end_date=$2 WHERE id=$1")
        .bind(id)
        .bind(day - Duration::days(28))
        .execute(&mut *locked)
        .await
        .unwrap();
    locked.commit().await.unwrap();
    assert_eq!(approval.await.unwrap().status(), 409);
    let state: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "cancelled");
}
