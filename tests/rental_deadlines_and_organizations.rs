#![cfg(feature = "ssr")]
mod common;
#[path = "rental_v2_support/mod.rs"]
mod support;

use axum::{body::Body, http::Request, response::Response};
use common::{App, body_json};
use obecni_web::backend::{auth, rentals, today};
use serde_json::{Value, json};
use time::{
    Date, Duration, OffsetDateTime,
    macros::{date, datetime},
};
use tower::ServiceExt;

async fn stored_rental(
    app: &App,
    asset: i64,
    day: Date,
    start: Option<i16>,
    end: Option<i16>,
    status: &str,
) -> i64 {
    sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,start_minute,end_minute,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter,identity,identity_type) VALUES ($1,$2,$2,$3,$4,'Testovací žadatel','tenant@example.test','123456789','Vyskeř 1',$5,100,'now','now',FALSE,$6,'image/png') RETURNING id")
        .bind(asset).bind(day).bind(start).bind(end).bind(status).bind(vec![1u8,2,3]).fetch_one(&app.state.pool).await.unwrap()
}
async fn state(app: &App, id: i64) -> String {
    sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}
async fn old_application(app: &App, id: i64) {
    sqlx::query("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,'tenant@example.test','Přijata','Staré potvrzení přijetí','application',0,0)")
        .bind(id).execute(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO rental_access_tokens(rental_id,token_hash) VALUES ($1,$2)")
        .bind(id)
        .bind(auth::hash(&format!("expired-{id}")))
        .execute(&app.state.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn decisions_supersede_both_application_messages_even_after_smtp_failure() {
    for (decision, purpose) in [("approve", "approval"), ("reject", "rejected")] {
        let app = App::new().await;
        let asset = support::asset(&app, false).await;
        let id = stored_rental(&app, asset, support::monday(), None, None, "pending").await;
        for recipient in ["tenant@example.test", "office@example.test"] {
            sqlx::query("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,$2,'Application','Awaiting approval','application',0,0)")
                .bind(id).bind(recipient).execute(&app.state.pool).await.unwrap();
        }
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let failed =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
                .port(9)
                .build();
        assert!(
            rentals::deliver_one(&app.state, &failed, now)
                .await
                .unwrap()
        );
        let attempted: i64 = sqlx::query_scalar("SELECT sum(attempts)::bigint FROM rental_mail_queue WHERE rental_id=$1 AND purpose='application'")
            .bind(id).fetch_one(&app.state.pool).await.unwrap();
        assert_eq!(attempted, 1);

        assert_eq!(
            app.call(
                "POST",
                &format!("/api/v1/admin/rentals/{id}/{decision}"),
                None,
                true
            )
            .await
            .status(),
            200
        );
        let cancelled: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose='application' AND cancelled_at IS NOT NULL AND sent_at IS NULL AND body IS NULL AND lock_token IS NULL AND locked_until=0")
            .bind(id).fetch_one(&app.state.pool).await.unwrap();
        assert_eq!(cancelled, 2);

        let (port, capture) = support::smtp_capture().await;
        let smtp =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
                .port(port)
                .build();
        let retry_time = now + Duration::hours(1);
        assert!(
            rentals::deliver_one(&app.state, &smtp, retry_time)
                .await
                .unwrap()
        );
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), capture)
            .await
            .unwrap()
            .unwrap();
        assert!(!message.contains("Awaiting approval"));
        let delivered: i64 = sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose=$2 AND sent_at IS NOT NULL")
            .bind(id).bind(purpose).fetch_one(&app.state.pool).await.unwrap();
        assert_eq!(delivered, 1);
        assert!(
            !rentals::deliver_one(&app.state, &smtp, retry_time)
                .await
                .unwrap()
        );
    }
}

#[tokio::test]
async fn delivery_discards_historical_receipts_and_approvals_for_superseded_states() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let failed =
        lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(9)
            .build();
    for (purpose, states) in [
        (
            "application",
            &[
                "approved",
                "rejected",
                "cancelled",
                "invoiced",
                "paid",
                "closed",
            ][..],
        ),
        (
            "approval",
            &[
                "pending",
                "rejected",
                "cancelled",
                "invoiced",
                "paid",
                "closed",
            ][..],
        ),
    ] {
        for status in states {
            let id = stored_rental(&app, asset, support::monday(), None, None, status).await;
            let message_id: i64 = sqlx::query_scalar("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,'tenant@example.test','Outdated','Outdated',$2,0,0) RETURNING id")
                .bind(id).bind(purpose).fetch_one(&app.state.pool).await.unwrap();
            assert!(
                rentals::deliver_one(&app.state, &failed, OffsetDateTime::now_utc())
                    .await
                    .unwrap()
            );
            let result: (bool, i64) = sqlx::query_as("SELECT cancelled_at IS NOT NULL AND sent_at IS NULL AND body IS NULL AND lock_token IS NULL AND locked_until=0,attempts FROM rental_mail_queue WHERE id=$1")
                .bind(message_id).fetch_one(&app.state.pool).await.unwrap();
            assert_eq!(
                result,
                (true, 0),
                "{purpose} for {status} must not reach SMTP"
            );
        }
    }
}

#[tokio::test]
async fn resumed_delivery_discards_ended_approvals_without_maintenance_but_sends_future_ones() {
    for (start, end) in [(None, None), (Some(1080), Some(1140))] {
        let app = App::new().await;
        let asset = support::asset(&app, true).await;
        let day = support::monday();
        let ended = stored_rental(&app, asset, day, start, end, "approved").await;
        let future =
            stored_rental(&app, asset, day + Duration::days(2), start, end, "approved").await;
        sqlx::query("UPDATE rentals SET require_meter=TRUE WHERE id=$1")
            .bind(ended)
            .execute(&app.state.pool)
            .await
            .unwrap();
        for (id, body) in [
            (ended, "Expired confirmation"),
            (future, "Future confirmation"),
        ] {
            sqlx::query("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,'tenant@example.test','Confirmation',$2,'approval',0,0)")
                .bind(id).bind(body).execute(&app.state.pool).await.unwrap();
        }
        let boundary: OffsetDateTime = sqlx::query_scalar(
            "SELECT ends_local AT TIME ZONE 'Europe/Prague' FROM rentals WHERE id=$1",
        )
        .bind(ended)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        // Simulate the mail worker resuming before any maintenance has run.
        let failed =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
                .port(9)
                .build();
        assert!(
            rentals::deliver_one(&app.state, &failed, boundary)
                .await
                .unwrap()
        );
        let discarded: (bool, i64) = sqlx::query_as("SELECT cancelled_at IS NOT NULL AND sent_at IS NULL AND body IS NULL AND lock_token IS NULL AND locked_until=0,attempts FROM rental_mail_queue WHERE rental_id=$1 AND purpose='approval'")
            .bind(ended).fetch_one(&app.state.pool).await.unwrap();
        assert_eq!(discarded, (true, 0));
        let preserved: bool = sqlx::query_scalar("SELECT status='approved' AND require_meter AND identity IS NOT NULL AND before_photo IS NULL AND after_photo IS NULL AND last_reminder_date IS NULL FROM rentals WHERE id=$1")
            .bind(ended).fetch_one(&app.state.pool).await.unwrap();
        assert!(
            preserved,
            "Discarding mail must leave the rental available for meter upload and settlement"
        );

        let (port, capture) = support::smtp_capture().await;
        let smtp =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
                .port(port)
                .build();
        assert!(
            rentals::deliver_one(&app.state, &smtp, boundary)
                .await
                .unwrap()
        );
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), capture)
            .await
            .unwrap()
            .unwrap();
        assert!(message.contains("Future confirmation"));
        assert!(!message.contains("Expired confirmation"));
        let sent: bool = sqlx::query_scalar("SELECT sent_at IS NOT NULL AND cancelled_at IS NULL FROM rental_mail_queue WHERE rental_id=$1 AND purpose='approval'")
            .bind(future).fetch_one(&app.state.pool).await.unwrap();
        assert!(sent);
    }
}

#[tokio::test]
async fn cancelled_admin_rental_never_retries_its_failed_approval() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let created = support::create(
        &app,
        support::admin_input(asset, support::monday(), Some("20:00"), Some("21:00")),
    )
    .await;
    let id = created["rentals"][0]["id"].as_i64().unwrap();
    let now = OffsetDateTime::now_utc() + Duration::seconds(1);
    let failed =
        lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(9)
            .build();
    assert!(
        rentals::deliver_one(&app.state, &failed, now)
            .await
            .unwrap()
    );
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
    let (port, capture) = support::smtp_capture().await;
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    let retry_time = now + Duration::hours(1);
    assert!(
        rentals::deliver_one(&app.state, &smtp, retry_time)
            .await
            .unwrap()
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), capture)
        .await
        .unwrap()
        .unwrap();
    let approval: (bool, i64) = sqlx::query_as("SELECT cancelled_at IS NOT NULL AND body IS NULL AND sent_at IS NULL,attempts FROM rental_mail_queue WHERE rental_id=$1 AND purpose='approval'")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(approval, (true, 1));
    let delivered: bool = sqlx::query_scalar("SELECT sent_at IS NOT NULL FROM rental_mail_queue WHERE rental_id=$1 AND purpose='cancellation'")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert!(delivered);
    assert!(
        !rentals::deliver_one(&app.state, &smtp, retry_time)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn pending_requests_expire_at_the_exact_prague_start_and_cleanup_only_once() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let day = date!(2030 - 01 - 10);
    let expired = stored_rental(&app, asset, day, Some(1080), Some(1140), "pending").await;
    let future = stored_rental(&app, asset, day, Some(1140), Some(1200), "pending").await;
    let approved = stored_rental(&app, asset, day, Some(1080), Some(1140), "approved").await;
    old_application(&app, expired).await;
    let boundary = datetime!(2030-01-10 17:00 UTC);
    rentals::maintenance(&app.state, boundary - Duration::nanoseconds(1))
        .await
        .unwrap();
    assert_eq!(state(&app, expired).await, "pending");
    rentals::maintenance(&app.state, boundary).await.unwrap();
    rentals::maintenance(&app.state, boundary).await.unwrap();
    assert_eq!(state(&app, expired).await, "cancelled");
    assert_eq!(state(&app, future).await, "pending");
    assert_eq!(state(&app, approved).await, "approved");
    let cleared: bool = sqlx::query_scalar(
        "SELECT identity IS NULL AND identity_type IS NULL FROM rentals WHERE id=$1",
    )
    .bind(expired)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(cleared);
    let tokens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rental_access_tokens WHERE rental_id=$1")
            .bind(expired)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(tokens, 0);
    let cancelled:bool=sqlx::query_scalar("SELECT cancelled_at IS NOT NULL AND body IS NULL FROM rental_mail_queue WHERE rental_id=$1 AND purpose='application'").bind(expired).fetch_one(&app.state.pool).await.unwrap();
    assert!(cancelled);
    let audit:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE entity_type='rental' AND entity_id=$1 AND operation='expired_unapproved' AND actor_id IS NULL").bind(expired).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(audit, 1);
    let mails:Vec<(String,String)>=sqlx::query_as("SELECT recipient,body FROM rental_mail_queue WHERE rental_id=$1 AND purpose='expired_unapproved'").bind(expired).fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].0, "tenant@example.test");
    assert!(mails[0].1.contains("nebyla potvrzena do začátku"));
}

#[tokio::test]
async fn whole_days_expire_at_local_midnight_and_disabled_maintenance_waits_for_reactivation() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let id = stored_rental(&app, asset, date!(2030 - 07 - 06), None, None, "pending").await;
    let boundary = datetime!(2030-07-05 22:00 UTC);
    rentals::maintenance(&app.state, boundary - Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(state(&app, id).await, "pending");
    sqlx::query("UPDATE rental_settings SET enabled=FALSE WHERE id=1")
        .execute(&app.state.pool)
        .await
        .unwrap();
    rentals::maintenance(&app.state, boundary).await.unwrap();
    assert_eq!(state(&app, id).await, "pending");
    sqlx::query("UPDATE rental_settings SET enabled=TRUE WHERE id=1")
        .execute(&app.state.pool)
        .await
        .unwrap();
    rentals::maintenance(&app.state, boundary).await.unwrap();
    assert_eq!(state(&app, id).await, "cancelled");
}

#[tokio::test]
async fn late_approval_cancels_a_whole_day_without_waiting_for_maintenance() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let id = stored_rental(
        &app,
        asset,
        today(OffsetDateTime::now_utc()),
        None,
        None,
        "pending",
    )
    .await;
    for _ in 0..2 {
        let response = app
            .call(
                "POST",
                &format!("/api/v1/admin/rentals/{id}/approve"),
                None,
                true,
            )
            .await;
        assert_eq!(response.status(), 409);
    }
    assert_eq!(state(&app, id).await, "cancelled");
    let approvals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose='approval'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(approvals, 0);
    let cancellations:i64=sqlx::query_scalar("SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose='expired_unapproved'").bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(cancellations, 1);
}

#[tokio::test]
async fn expired_requests_do_not_block_availability_or_new_writes_and_cannot_send_stale_acceptance()
{
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let day = today(OffsetDateTime::now_utc());
    let id = stored_rental(&app, asset, day, None, None, "pending").await;
    old_application(&app, id).await;
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
    assert_eq!(availability, json!([]));
    let response = app
        .call(
            "POST",
            &format!("/api/v1/admin/rental-assets/{asset}/blocks"),
            Some(json!({"start_date":day.to_string(),"end_date":day.to_string()})),
            true,
        )
        .await;
    assert_eq!(response.status(), 201);
    assert_eq!(state(&app, id).await, "cancelled");

    let second = stored_rental(&app, asset, day, None, None, "pending").await;
    old_application(&app, second).await;
    // Only the second stale application is due, so no SMTP connection should be attempted.
    sqlx::query("UPDATE rental_mail_queue SET next_attempt_at=$1 WHERE rental_id=$2")
        .bind(i64::MAX / 2)
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let smtp = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(9)
        .build();
    assert!(
        rentals::deliver_one(&app.state, &smtp, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    assert_eq!(state(&app, second).await, "cancelled");
}

fn public_input(asset: i64) -> Value {
    let day = support::monday().to_string();
    json!({"asset_id":asset.to_string(),"start_date":day,"end_date":day,"start_time":"18:00","end_time":"19:00","tenant_type":"organization","name":"Zkušební spolek","organization_ico":"12345679","responsible_name":"Jana Odpovědná","email":"tenant@example.test","phone":"123456789","event_type":"nonprofit","consent":"true"})
}
async fn public_upload(app: &App, fields: Value, identity: bool) -> Response {
    let mut body = Vec::new();
    for (key, value) in fields.as_object().unwrap() {
        body.extend_from_slice(
            format!(
                "--public-boundary\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{}\r\n",
                value.as_str().unwrap()
            )
            .as_bytes(),
        );
    }
    if identity {
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(3, 3)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        body.extend_from_slice(b"--public-boundary\r\nContent-Disposition: form-data; name=\"identity\"; filename=\"test.png\"\r\nContent-Type: image/png\r\n\r\n");
        body.extend_from_slice(image.get_ref());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--public-boundary--\r\n");
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/rentals")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=public-boundary",
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn organization_request_uses_ico_and_responsible_person_instead_of_a_registered_address() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let response = public_upload(&app, public_input(asset), true).await;
    let status = response.status();
    let created = body_json(response).await;
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_i64().unwrap();
    let detail = body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
            .await,
    )
    .await;
    assert_eq!(detail["name"], "Zkušební spolek");
    assert_eq!(detail["organization_ico"], "12345679");
    assert_eq!(detail["responsible_name"], "Jana Odpovědná");
    assert_eq!(detail["address"], "");
    assert_eq!(detail["identity_uploaded"], true);
    let response = app
        .call(
            "POST",
            &format!("/api/v1/admin/rentals/{id}/identity"),
            Some(json!({"password":"Testovaci-dlouhe-heslo-123456!"})),
            true,
        )
        .await;
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn backend_requires_organization_identifiers_a_real_identity_upload_and_consent() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let valid = public_input(asset);
    let mut inputs = Vec::new();
    for ico in ["", "12345678", "1234567", "１２３４５６７９"] {
        let mut data = valid.clone();
        data["organization_ico"] = json!(ico);
        inputs.push((data, true));
    }
    let mut missing_name = valid.clone();
    missing_name
        .as_object_mut()
        .unwrap()
        .remove("responsible_name");
    inputs.push((missing_name, true));
    inputs.push((valid.clone(), false));
    let mut no_consent = valid.clone();
    no_consent["consent"] = json!("false");
    inputs.push((no_consent, true));
    let mut person = valid;
    person["tenant_type"] = json!("person");
    person.as_object_mut().unwrap().remove("organization_ico");
    person.as_object_mut().unwrap().remove("responsible_name");
    inputs.push((person, true));
    for (input, identity) in inputs {
        let response = public_upload(&app, input, identity).await;
        assert_eq!(response.status(), 400);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rentals")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn admin_organizations_require_an_upload_and_each_repeated_occurrence_keeps_its_identity_lifecycle()
 {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let day = support::monday();
    let mut input = support::admin_input(asset, day, Some("18:00"), Some("19:00"));
    input["tenant_type"] = json!("organization");
    input["organization_ico"] = json!("12345679");
    input["responsible_name"] = json!("Jana Odpovědná");
    input["consent"] = json!(true);
    input["recurrence_interval_days"] = json!(7);
    input["recurrence_until"] = json!((day + Duration::days(7)).to_string());
    input.as_object_mut().unwrap().remove("address");
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
        app.call("POST", "/api/v1/admin/rentals", Some(input.clone()), true)
            .await
            .status(),
        400
    );
    assert_eq!(
        support::admin_upload(&app, input.clone(), false)
            .await
            .status(),
        400
    );
    let mut no_consent = input.clone();
    no_consent["consent"] = json!(false);
    assert_eq!(
        support::admin_upload(&app, no_consent, true).await.status(),
        400
    );
    let response = support::admin_upload(&app, input, true).await;
    let status = response.status();
    let created = body_json(response).await;
    assert_eq!(status, 201, "{created}");
    let rows = created["rentals"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for rental in rows {
        assert_eq!(rental["identity_uploaded"], true);
        assert_eq!(rental["organization_ico"], "12345679");
        assert_eq!(rental["responsible_name"], "Jana Odpovědná");
        assert_eq!(rental["address"], "");
    }
    let id = rows[0]["id"].as_i64().unwrap();
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
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rentals WHERE identity IS NOT NULL")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn historical_organizations_remain_readable_without_invented_identifiers() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let id = stored_rental(
        &app,
        asset,
        support::monday(),
        Some(1080),
        Some(1140),
        "approved",
    )
    .await;
    sqlx::query("UPDATE rentals SET tenant_type='organization' WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let detail = body_json(
        app.call("GET", &format!("/api/v1/admin/rentals/{id}"), None, true)
            .await,
    )
    .await;
    assert!(detail["organization_ico"].is_null());
    assert!(detail["responsible_name"].is_null());
    assert_eq!(detail["address"], "Vyskeř 1");
}

#[tokio::test]
async fn queued_acceptance_rechecks_the_clock_after_waiting_for_the_rental_lock() {
    use time_tz::{PrimitiveDateTimeExt, timezones::db::europe::PRAGUE};
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let day = today(OffsetDateTime::now_utc());
    let id = stored_rental(&app, asset, day, None, None, "pending").await;
    old_application(&app, id).await;
    let stale_now = day.midnight().assume_timezone(PRAGUE).unwrap() - Duration::seconds(1);
    let mut lock = app.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM rentals WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let backend = app.state.clone();
    let sender = tokio::spawn(async move {
        let smtp =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous("127.0.0.1")
                .port(9)
                .build();
        rentals::deliver_one(&backend, &smtp, stale_now)
            .await
            .unwrap()
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!sender.is_finished());
    lock.commit().await.unwrap();
    assert!(sender.await.unwrap());
    assert_eq!(state(&app, id).await, "cancelled");
    let attempted: i64 = sqlx::query_scalar(
        "SELECT attempts FROM rental_mail_queue WHERE rental_id=$1 AND purpose='application'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(attempted, 0, "The stale acceptance must never reach SMTP");
}

#[tokio::test]
async fn rejection_reloads_a_changed_period_after_waiting_for_its_lock() {
    let app = App::new().await;
    let asset = support::asset(&app, false).await;
    let day = support::monday();
    let id = stored_rental(&app, asset, day, Some(1080), Some(1140), "pending").await;
    let mut lock = app.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM rentals WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/rentals/{id}/reject"))
        .header("cookie", &app.cookie)
        .header("x-csrf-token", &app.csrf)
        .body(Body::empty())
        .unwrap();
    let router = app.router.clone();
    let action = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!action.is_finished());
    sqlx::query("UPDATE rentals SET start_date=$2,end_date=$2 WHERE id=$1")
        .bind(id)
        .bind(day - Duration::days(28))
        .execute(&mut *lock)
        .await
        .unwrap();
    lock.commit().await.unwrap();
    assert_eq!(action.await.unwrap().status(), 409);
    assert_eq!(state(&app, id).await, "cancelled");
    let wrong_mail: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rental_mail_queue WHERE rental_id=$1 AND purpose='rejected'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(wrong_mail, 0);
}
