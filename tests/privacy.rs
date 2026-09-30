#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::backend::{Backend, privacy, subscriptions};
use serde_json::json;
use time::{Duration, OffsetDateTime};

#[tokio::test]
async fn subscription_must_match_the_displayed_wording() {
    let app = App::new().await;
    for consent in [json!({"fingerprint":""}), json!({"fingerprint":"outdated"})] {
        let response = app
            .call(
                "POST",
                "/api/v1/subscriptions",
                Some(json!({"email":"person@example.test","consent":consent})),
                false,
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM subscribers")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut config = (*app.state.config).clone();
    config.privacy = None;
    let unavailable = Backend::new(app.state.pool.clone(), config);
    assert_eq!(
        subscriptions::request_subscription(
            &unavailable,
            "person@example.test",
            &common::consent(&app.state),
            OffsetDateTime::now_utc()
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn consent_snapshot_is_immutable_and_unsubscribe_remains_available() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "person@example.test", now)
        .await
        .unwrap();
    let token = app.confirmation_token("person@example.test").await;
    let response = app
        .call(
            "GET",
            &format!("/odber/potvrdit?token={token}"),
            None,
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let pending: Option<i64> = sqlx::query_scalar("SELECT confirmed_at FROM subscription_consents")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(pending.is_none());
    subscriptions::use_token(&app.state, &token, true, now)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE consent_notices SET consent_text='changed'")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE subscription_consents SET requested_at=0")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    let subscriber: i64 = sqlx::query_scalar("SELECT id FROM subscribers")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let path = format!("/api/v1/admin/subscribers/{subscriber}/consents");
    assert_eq!(
        app.call("GET", &path, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let evidence = body_json(app.call("GET", &path, None, true).await).await;
    assert_eq!(
        evidence[0]["consent_text"],
        app.state.config.privacy.as_ref().unwrap().consent_text()
    );
    assert_eq!(evidence[0]["confirmed_at"], now.unix_timestamp());
    app.publish(app.notice(json!({})).await).await;
    let body: String = sqlx::query_scalar("SELECT body FROM mail_queue WHERE purpose='document'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let unsubscribe = common::token_from_body(&body);
    subscriptions::use_token(&app.state, &unsubscribe, false, now + Duration::days(400))
        .await
        .unwrap();
    let withdrawn: i64 = sqlx::query_scalar("SELECT withdrawn_at FROM subscription_consents")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(withdrawn, (now + Duration::days(400)).unix_timestamp());
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_queue WHERE cancelled=FALSE AND sent_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn legacy_verified_addresses_are_not_treated_as_proven_consent() {
    let app = App::new().await;
    sqlx::query("INSERT INTO subscribers(email,verified_at,retention_started_at) VALUES ('legacy@example.test',1,1)").execute(&app.state.pool).await.unwrap();
    app.publish(app.notice(json!({})).await).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    common::request_subscription(&app.state, "legacy@example.test", OffsetDateTime::now_utc())
        .await
        .unwrap();
    let token = app.confirmation_token("legacy@example.test").await;
    subscriptions::use_token(&app.state, &token, true, OffsetDateTime::now_utc())
        .await
        .unwrap();
    app.publish(app.notice(json!({})).await).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE purpose='document'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn retention_deletes_expired_data_but_keeps_active_subscriptions_and_audit_protection() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "pending@example.test", now - Duration::days(3))
        .await
        .unwrap();
    common::request_subscription(&app.state, "active@example.test", now - Duration::days(3))
        .await
        .unwrap();
    let token = app.confirmation_token("active@example.test").await;
    subscriptions::use_token(&app.state, &token, true, now - Duration::days(3))
        .await
        .unwrap();
    sqlx::query("INSERT INTO audit_log(occurred_at,operation,entity_type) VALUES ('2020-01-01T00:00:00Z','old','privacy')").execute(&app.state.pool).await.unwrap();
    assert!(
        sqlx::query("DELETE FROM audit_log")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    privacy::maintenance(&app.state, now).await.unwrap();
    let emails: Vec<String> = sqlx::query_scalar("SELECT email FROM subscribers")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(emails, vec!["active@example.test"]);
    let old: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE operation='old'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(old, 0);
    assert!(
        sqlx::query("DELETE FROM audit_log")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE audit_log SET operation='changed'")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
}

#[test]
fn privacy_configuration_validates_dates_and_retention() {
    let mut policy: obecni_web::privacy::PrivacyPolicy =
        serde_json::from_str(include_str!("../config/privacy.example.json")).unwrap();
    policy.validate().unwrap();
    policy.approved_on = Some("2026-01-01".into());
    policy.validate().unwrap();
    policy.retention.pending_days = 0;
    assert!(policy.validate().is_err());
}

#[tokio::test]
async fn internal_notice_data_expires_without_erasing_the_public_record() {
    let app = App::new().await;
    let id=app.notice(json!({"title":"Osobní údaje","description":"Interní popis","reference_number":"osobní reference"})).await;
    app.publish(id).await;
    app.call(
        "POST",
        &format!("/api/v1/admin/notices/{id}/withdraw"),
        None,
        true,
    )
    .await;
    let now = OffsetDateTime::now_utc() + Duration::days(400);
    privacy::maintenance(&app.state, now).await.unwrap();
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT title,description FROM notices WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(row, (format!("Záznam úřední desky #{id}"), None));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notice_events WHERE notice_id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let other = app.notice(json!({})).await;
    app.publish(other).await;
    assert!(
        sqlx::query("DELETE FROM notice_events")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM notices WHERE id=$1")
            .bind(id)
            .execute(&app.state.pool)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn confirmation_can_render_a_policy_snapshot_from_before_a_config_extension() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut old = serde_json::to_value(app.state.config.privacy.as_ref().unwrap()).unwrap();
    old["retention"]
        .as_object_mut()
        .unwrap()
        .remove("notice_internal_days");
    let subscriber = sqlx::query_scalar::<_, i64>("INSERT INTO subscribers(email,retention_started_at) VALUES ('old-snapshot@vysker.test',$1) RETURNING id")
    .bind(now)
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
    ;
    sqlx::query("INSERT INTO consent_notices(fingerprint,version,consent_text,privacy_json) VALUES ('old-snapshot','old-version','Přihlašuji se k odběru.',$1)").bind(old.to_string()).execute(&app.state.pool).await.unwrap();
    let consent=sqlx::query_scalar::<_, i64>("INSERT INTO subscription_consents(subscriber_id,notice_fingerprint,requested_at) VALUES ($1,'old-snapshot',$2) RETURNING id").bind(subscriber).bind(now).fetch_one(&app.state.pool).await.unwrap();
    let token = obecni_web::backend::auth::token();
    sqlx::query("INSERT INTO subscription_tokens(hash,subscriber_id,consent_id,purpose,expires_at) VALUES ($1,$2,$3,'verification',$4)").bind(obecni_web::backend::auth::hash(&token)).bind(subscriber).bind(consent).bind(now+86400).execute(&app.state.pool).await.unwrap();
    assert_eq!(
        app.call(
            "GET",
            &format!("/odber/potvrdit?token={token}"),
            None,
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
}
