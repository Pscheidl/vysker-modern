#![cfg(feature = "ssr")]
mod common;
#[path = "rental_v2_support/mod.rs"]
mod support;

use axum::{body::Body, http::Request, response::Response};
use common::{App, body_json};
use serde_json::{Value, json};
use time::{Date, Duration};
use tower::ServiceExt;

fn rates() -> Value {
    json!({
        "nonprofit":{"hourly_price":20,"weekend_day_price":1000},
        "standard":{"hourly_price":200,"weekend_day_price":6000}
    })
}
fn asset_input(rates: Value) -> Value {
    json!({"name":"Testovací sál","description":"Ceník","notification_email":"office@example.test","require_meter":true,"active":true,"rates":rates})
}
async fn asset(app: &App, rates: Value) -> i64 {
    let response = app
        .call(
            "POST",
            "/api/v1/admin/rental-assets",
            Some(asset_input(rates)),
            true,
        )
        .await;
    let status = response.status();
    let value = body_json(response).await;
    assert_eq!(status, 200, "{value}");
    value["id"].as_i64().unwrap()
}
async fn request(
    app: &App,
    asset: i64,
    from: Date,
    to: Date,
    hours: Option<(&str, &str)>,
    metadata: &[(&str, &str)],
) -> Response {
    let mut image = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(3, 3)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    let mut fields = vec![
        ("asset_id", asset.to_string()),
        ("start_date", from.to_string()),
        ("end_date", to.to_string()),
        ("name", "Testovací nájemce".into()),
        ("email", "tenant@example.test".into()),
        ("phone", "123456789".into()),
        ("address", "Vyskeř 1".into()),
        ("consent", "true".into()),
    ];
    if let Some((start, end)) = hours {
        fields.extend([("start_time", start.into()), ("end_time", end.into())]);
    }
    if metadata.contains(&("tenant_type", "organization")) {
        fields.extend([
            ("organization_ico", "12345679".into()),
            ("responsible_name", "Jana Odpovědná".into()),
        ]);
    }
    fields.extend(
        metadata
            .iter()
            .map(|(key, value)| (*key, (*value).to_owned())),
    );
    let mut body = Vec::new();
    for (key, value) in fields {
        body.extend_from_slice(
            format!(
                "--boundary\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"identity\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n");
    body.extend_from_slice(image.get_ref());
    body.extend_from_slice(b"\r\n--boundary--\r\n");
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/rentals")
                .header("content-type", "multipart/form-data; boundary=boundary")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn detail(app: &App, response: Response) -> Value {
    let status = response.status();
    let value = body_json(response).await;
    assert_eq!(status, 201, "{value}");
    let id = value["id"].as_i64().unwrap();
    body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
            .await,
    )
    .await
}

#[tokio::test]
async fn event_type_alone_selects_server_price_for_people_and_organizations() {
    let app = App::new().await;
    let asset = asset(&app, rates()).await;
    let monday = support::monday();
    for (index, tenant, event, price) in [
        (0, "person", "nonprofit", 40),
        (1, "organization", "nonprofit", 40),
        (2, "person", "standard", 400),
        (3, "organization", "standard", 400),
    ] {
        let day = monday + Duration::days(index * 7);
        let rental = detail(
            &app,
            request(
                &app,
                asset,
                day,
                day,
                Some(("18:00", "20:00")),
                &[("tenant_type", tenant), ("event_type", event)],
            )
            .await,
        )
        .await;
        assert_eq!(rental["rental_price"], price);
        assert_eq!(rental["event_type"], event);
        assert_eq!(rental["rate_group"], event);
        assert_eq!(rental["tenant_type"], tenant);
        assert_eq!(rental["rate_snapshot"], rates()[event]);
    }
    let public = body_json(app.call("GET", "/api/v1/rental-assets", None, false).await).await;
    assert_eq!(public[0]["rates"], rates());
    for obsolete in [
        "weekday_price",
        "weekend_price",
        "hourly_price",
        "weekend_day_price",
    ] {
        assert!(public[0].get(obsolete).is_none());
    }
    let availability = body_json(
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
    for private in [
        "tenant_type",
        "event_type",
        "is_local",
        "rate_group",
        "rate_snapshot",
        "rental_price",
        "nájemce",
        "tenant@example.test",
    ] {
        assert!(!availability.contains(private), "{availability}");
    }
}

#[tokio::test]
async fn the_same_daily_price_applies_on_weekdays_and_weekends() {
    let app = App::new().await;
    let asset = asset(&app, rates()).await;
    let monday = support::monday();
    let event = [("event_type", "standard")];
    for day in [monday, monday + Duration::days(4)] {
        assert_eq!(
            detail(&app, request(&app, asset, day, day, None, &event).await).await["rental_price"],
            6000
        );
    }
    let saturday = monday + Duration::days(5);
    let sunday = saturday + Duration::days(1);
    for day in [saturday, sunday] {
        assert_eq!(
            request(&app, asset, day, day, Some(("18:00", "19:00")), &event)
                .await
                .status(),
            400
        );
    }
    for day in [saturday, sunday] {
        assert_eq!(
            detail(&app, request(&app, asset, day, day, None, &event).await).await["rental_price"],
            6000
        );
    }
    let next = saturday + Duration::days(7);
    assert_eq!(
        detail(
            &app,
            request(&app, asset, next, next + Duration::days(1), None, &event).await
        )
        .await["rental_price"],
        12000
    );
}

#[tokio::test]
async fn consecutive_whole_days_allow_mixed_weeks_up_to_28_days_and_keep_hourly_pricing_distinct() {
    let app = App::new().await;
    let asset = asset(&app, rates()).await;
    let monday = support::monday();
    let event = [("event_type", "standard")];
    for (start_offset, days) in [(4, 2), (13, 2), (19, 9), (35, 28)] {
        let from = monday + Duration::days(start_offset);
        let to = from + Duration::days(days - 1);
        let rental = detail(&app, request(&app, asset, from, to, None, &event).await).await;
        assert_eq!(rental["rental_price"], days * 6000);
        assert!(rental["start_time"].is_null());
        assert!(rental["end_time"].is_null());
    }
    assert_eq!(
        request(
            &app,
            asset,
            monday,
            monday + Duration::days(28),
            None,
            &event
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        request(
            &app,
            asset,
            monday,
            monday + Duration::days(1),
            Some(("18:00", "19:00")),
            &event
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        detail(
            &app,
            request(
                &app,
                asset,
                monday,
                monday,
                Some(("00:00", "24:00")),
                &event
            )
            .await
        )
        .await["rental_price"],
        4800
    );
}

#[tokio::test]
async fn public_daily_ranges_cannot_bridge_an_occupied_middle_hour() {
    let app = App::new().await;
    let asset = asset(&app, rates()).await;
    let monday = support::monday();
    let tuesday = monday + Duration::days(1);
    let wednesday = monday + Duration::days(2);
    let event = [("event_type", "standard")];
    assert_eq!(
        request(
            &app,
            asset,
            tuesday,
            tuesday,
            Some(("10:00", "11:00")),
            &event
        )
        .await
        .status(),
        201
    );
    assert_eq!(
        request(&app, asset, monday, wednesday, None, &event)
            .await
            .status(),
        409
    );
    assert_eq!(
        request(&app, asset, monday, monday, None, &event)
            .await
            .status(),
        201
    );
    assert_eq!(
        request(&app, asset, wednesday, wednesday, None, &event)
            .await
            .status(),
        201
    );
    assert_eq!(
        request(
            &app,
            asset,
            tuesday,
            tuesday,
            Some(("09:00", "10:00")),
            &event
        )
        .await
        .status(),
        201
    );
}

#[tokio::test]
async fn null_prices_disable_public_modes_while_zero_is_valid_and_private_agreements_work() {
    let app = App::new().await;
    let configured = json!({"nonprofit":{"hourly_price":null,"weekend_day_price":0},"standard":{"hourly_price":0,"weekend_day_price":null}});
    let asset = asset(&app, configured.clone()).await;
    let day = support::monday();
    let saturday = day + Duration::days(5);
    assert_eq!(
        request(&app, asset, day, day, None, &[("event_type", "standard")])
            .await
            .status(),
        400
    );
    let tuesday = day + Duration::days(1);
    assert_eq!(
        detail(
            &app,
            request(
                &app,
                asset,
                tuesday,
                tuesday,
                None,
                &[("event_type", "nonprofit")]
            )
            .await
        )
        .await["rental_price"],
        0
    );
    assert_eq!(
        request(
            &app,
            asset,
            day,
            day,
            Some(("18:00", "19:00")),
            &[("event_type", "nonprofit")]
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        request(
            &app,
            asset,
            saturday,
            saturday,
            None,
            &[("event_type", "standard")]
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        detail(
            &app,
            request(
                &app,
                asset,
                day,
                day,
                Some(("18:00", "19:00")),
                &[("event_type", "standard")]
            )
            .await
        )
        .await["rental_price"],
        0
    );
    assert_eq!(
        detail(
            &app,
            request(
                &app,
                asset,
                saturday,
                saturday,
                None,
                &[("event_type", "nonprofit")]
            )
            .await
        )
        .await["rental_price"],
        0
    );
    let mut admin =
        support::admin_input(asset, day + Duration::days(2), Some("20:00"), Some("21:00"));
    admin["event_type"] = json!("nonprofit");
    admin["rental_price"] = json!(17);
    let created = support::create(&app, admin).await;
    assert_eq!(created["rentals"][0]["rental_price"], 17);
    assert!(created["rentals"][0]["rate_snapshot"]["hourly_price"].is_null());
    assert_eq!(
        body_json(app.call("GET", "/api/v1/rental-assets", None, false).await).await[0]["rates"],
        configured
    );
}

#[tokio::test]
async fn price_changes_and_event_model_do_not_rewrite_existing_agreements() {
    let app = App::new().await;
    let asset = asset(&app, rates()).await;
    let monday = support::monday();
    let rental = detail(
        &app,
        request(
            &app,
            asset,
            monday,
            monday,
            Some(("18:00", "20:00")),
            &[("event_type", "nonprofit")],
        )
        .await,
    )
    .await;
    let id = rental["id"].as_i64().unwrap();
    let mut changed = rates();
    changed["nonprofit"]["hourly_price"] = json!(999);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/rental-assets/{asset}"),
            Some(asset_input(changed)),
            true
        )
        .await
        .status(),
        200
    );
    let unchanged = body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
            .await,
    )
    .await;
    assert_eq!(unchanged["rental_price"], 40);
    assert_eq!(unchanged["rate_snapshot"]["hourly_price"], 20);
    let old_snapshot =
        json!({"enabled":false,"weekday_price":0,"weekend_price":0,"hourly_price":0});
    let legacy:i64=sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter,tenant_type,is_local,locality_source,rate_group,rate_snapshot) VALUES ($1,$2,$2,'Starší rezervace','legacy@example.test','123456789','Vyskeř 1','pending',777,'now','now',FALSE,'organization',TRUE,'manual','local_organization',$3) RETURNING id").bind(asset).bind(monday+Duration::days(2)).bind(sqlx::types::Json(old_snapshot.clone())).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/rentals/{legacy}/approve"),
            None,
            true
        )
        .await
        .status(),
        200
    );
    let original = body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/rentals/{legacy}"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(original["rental_price"], 777);
    assert_eq!(original["rate_group"], "local_organization");
    assert_eq!(original["is_local"], true);
    assert_eq!(original["rate_snapshot"], old_snapshot);
    assert!(original["event_type"].is_null());
}

#[tokio::test]
async fn old_price_payloads_and_missing_or_invalid_event_types_are_rejected_without_mutation() {
    let app = App::new().await;
    let asset = asset(&app, rates()).await;
    let day = support::monday();
    for metadata in [
        vec![],
        vec![("event_type", "external")],
        vec![("event_type", "standard"), ("tenant_type", "administrator")],
        vec![("event_type", "standard"), ("is_local", "true")],
        vec![("event_type", "standard"), ("locality_source", "manual")],
        vec![("event_type", "standard"), ("rental_price", "1")],
    ] {
        assert_eq!(
            request(&app, asset, day, day, Some(("18:00", "19:00")), &metadata)
                .await
                .status(),
            400
        );
    }
    let before = body_json(
        app.call("GET", "/api/v1/admin/rental-assets", None, true)
            .await,
    )
    .await;
    let mut legacy = asset_input(rates());
    legacy.as_object_mut().unwrap().remove("rates");
    legacy["weekday_price"] = json!(17);
    legacy["weekend_price"] = json!(19);
    for (method, path) in [
        ("PUT", format!("/api/v1/admin/rental-assets/{asset}")),
        ("POST", "/api/v1/admin/rental-assets".into()),
    ] {
        assert_eq!(
            app.call(method, &path, Some(legacy.clone()), true)
                .await
                .status(),
            400
        );
    }
    let mut ambiguous = asset_input(rates());
    ambiguous["weekend_price"] = Value::Null;
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/rental-assets/{asset}"),
            Some(ambiguous),
            true
        )
        .await
        .status(),
        400
    );
    for obsolete in [
        json!({"external":{"hourly_price":5}}),
        json!({"standard":{"hourly_price":5,"enabled":false}}),
        json!({"standard":{"weekend_price":5}}),
    ] {
        assert_eq!(
            app.call(
                "PUT",
                &format!("/api/v1/admin/rental-assets/{asset}"),
                Some(asset_input(obsolete)),
                true
            )
            .await
            .status(),
            422
        );
    }
    let mut invalid = rates();
    invalid["nonprofit"]["hourly_price"] = json!(-1);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/rental-assets/{asset}"),
            Some(asset_input(invalid)),
            true
        )
        .await
        .status(),
        400
    );
    let after = body_json(
        app.call("GET", "/api/v1/admin/rental-assets", None, true)
            .await,
    )
    .await;
    assert_eq!(before, after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rentals")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn current_price_changes_preserve_existing_legacy_prices_and_rental_snapshots() {
    let app = App::new().await;
    let asset = asset(&app, json!({"nonprofit":{},"standard":{}})).await;
    sqlx::query("UPDATE rental_assets SET weekday_price=120,weekend_price=555,hourly_price=25,weekend_day_price=444 WHERE id=$1")
        .bind(asset).execute(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO rental_asset_rates(asset_id,rate_group,weekday_price,weekend_price,hourly_price,enabled) VALUES ($1,'external',2000,6000,200,FALSE)")
        .bind(asset).execute(&app.state.pool).await.unwrap();
    let rental:i64=sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter,rate_snapshot) VALUES ($1,'2026-10-30','2026-11-01','Existing','old@example.test','123456789','Vyskeř 1','approved',8123,'now','now',FALSE,$2) RETURNING id")
        .bind(asset).bind(sqlx::types::Json(json!({"weekday_price":2000,"weekend_price":6000,"hourly_price":200,"enabled":false}))).fetch_one(&app.state.pool).await.unwrap();
    let unset:(i64,bool)=sqlx::query_as("SELECT count(*),bool_and(hourly_price IS NULL AND weekend_day_price IS NULL) FROM rental_event_rates WHERE asset_id=$1")
        .bind(asset).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(unset, (2, true));
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/rental-assets/{asset}"),
            Some(asset_input(rates())),
            true
        )
        .await
        .status(),
        200
    );
    let legacy:(Option<i64>,Option<i64>,Option<i64>,bool)=sqlx::query_as("SELECT weekday_price,weekend_price,hourly_price,enabled FROM rental_asset_rates WHERE asset_id=$1 AND rate_group='external'")
        .bind(asset).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(legacy, (Some(2000), Some(6000), Some(200), false));
    let original = body_json(
        app.call(
            "GET",
            &format!("/api/v1/admin/rentals/{rental}"),
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(original["rental_price"], 8123);
    assert_eq!(original["rate_snapshot"]["weekend_price"], 6000);
    assert!(original["event_type"].is_null());
}

#[tokio::test]
async fn administrator_recurrence_requires_event_type_and_keeps_the_private_price() {
    let app = App::new().await;
    let asset = asset(&app, json!({"nonprofit":{},"standard":{}})).await;
    let monday = support::monday();
    let mut admin = support::admin_input(asset, monday, Some("20:00"), Some("21:00"));
    admin.as_object_mut().unwrap().remove("event_type");
    admin["recurrence_interval_days"] = json!(7);
    admin["recurrence_until"] = json!((monday + Duration::days(7)).to_string());
    for path in ["/api/v1/admin/rentals/preview", "/api/v1/admin/rentals"] {
        assert_eq!(
            app.call("POST", path, Some(admin.clone()), true)
                .await
                .status(),
            422
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rental_series")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
    admin["event_type"] = json!("nonprofit");
    admin["tenant_type"] = json!("organization");
    admin["organization_ico"] = json!("12345679");
    admin["responsible_name"] = json!("Jana Odpovědná");
    admin["consent"] = json!(true);
    admin["rental_price"] = json!(17);
    let response = support::admin_upload(&app, admin, true).await;
    let status = response.status();
    let created = body_json(response).await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["rentals"].as_array().unwrap().len(), 2);
    for occurrence in created["rentals"].as_array().unwrap() {
        assert_eq!(occurrence["event_type"], "nonprofit");
        assert_eq!(occurrence["rental_price"], 17);
        assert_eq!(
            occurrence["rate_snapshot"],
            json!({"hourly_price":null,"weekend_day_price":null})
        );
    }
}
