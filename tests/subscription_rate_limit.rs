#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::App;
use obecni_web::backend::{Backend, privacy, subscriptions};
use serde_json::json;
use time::{Duration, OffsetDateTime};

async fn evidence_counts(state: &Backend) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
         (SELECT count(*) FROM mail_queue WHERE purpose='verification'),
         (SELECT count(*) FROM subscription_consents),
         (SELECT count(*) FROM subscription_tokens WHERE purpose='verification'),
         (SELECT count(*) FROM audit_log WHERE operation='subscription_requested')",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

async fn mark_delivered(state: &Backend, now: OffsetDateTime) {
    sqlx::query(
        "UPDATE mail_queue SET sent_at=$1,body=NULL
         WHERE purpose='verification' AND cancelled=FALSE AND sent_at IS NULL",
    )
    .bind(now.unix_timestamp())
    .execute(&state.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn repeated_and_concurrent_requests_preserve_one_pending_confirmation() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let mut requests = tokio::task::JoinSet::new();
    for attempt in 0..12 {
        let state = app.state.clone();
        requests.spawn(async move {
            common::request_subscription(
                &state,
                if attempt % 2 == 0 {
                    "citizen@example.test"
                } else {
                    " CITIZEN@example.TEST "
                },
                now,
            )
            .await
            .unwrap();
        });
    }
    while let Some(result) = requests.join_next().await {
        result.unwrap();
    }
    assert_eq!(evidence_counts(&app.state).await, (1, 1, 1, 1));
    let token = app.confirmation_token("citizen@example.test").await;
    common::request_subscription(
        &app.state,
        " Citizen@Example.Test ",
        now + Duration::seconds(1),
    )
    .await
    .unwrap();
    assert_eq!(evidence_counts(&app.state).await, (1, 1, 1, 1));
    assert_eq!(app.confirmation_token("citizen@example.test").await, token);
    let retained: i64 = sqlx::query_scalar("SELECT retention_started_at FROM subscribers")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(retained, now.unix_timestamp());
    subscriptions::use_token(&app.state, &token, true, now + Duration::seconds(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn queued_retries_and_leases_suppress_resends_until_the_link_expires() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "waiting@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let original = app.confirmation_token(email).await;
    sqlx::query(
        "UPDATE mail_queue SET attempts=4,next_attempt_at=$1,locked_until=$2,lock_token='worker'",
    )
    .bind((now + Duration::hours(2)).unix_timestamp())
    .bind((now + Duration::minutes(65)).unix_timestamp())
    .execute(&app.state.pool)
    .await
    .unwrap();
    common::request_subscription(&app.state, email, now + Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(evidence_counts(&app.state).await, (1, 1, 1, 1));
    assert_eq!(app.confirmation_token(email).await, original);

    let expired = now + Duration::days(1);
    common::request_subscription(&app.state, email, expired)
        .await
        .unwrap();
    let replacement = app.confirmation_token(email).await;
    assert_ne!(replacement, original);
    assert_eq!(evidence_counts(&app.state).await, (2, 2, 1, 2));
    let cancelled: bool =
        sqlx::query_scalar("SELECT cancelled AND body IS NULL FROM mail_queue ORDER BY id LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(cancelled);
    assert!(
        subscriptions::use_token(&app.state, &original, true, expired)
            .await
            .is_err()
    );
    subscriptions::use_token(&app.state, &replacement, true, expired)
        .await
        .unwrap();
}

#[tokio::test]
async fn delivered_confirmation_has_cooldown_and_resend_keeps_the_original_link_valid() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "delivered@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let original = app.confirmation_token(email).await;
    mark_delivered(&app.state, now).await;
    for seconds in [60, 599] {
        common::request_subscription(&app.state, email, now + Duration::seconds(seconds))
            .await
            .unwrap();
        assert_eq!(evidence_counts(&app.state).await, (1, 1, 1, 1));
    }
    common::request_subscription(&app.state, email, now + Duration::minutes(10))
        .await
        .unwrap();
    let resent = app.confirmation_token(email).await;
    assert_ne!(resent, original);
    assert_eq!(evidence_counts(&app.state).await, (2, 1, 2, 2));
    subscriptions::use_token(&app.state, &original, true, now + Duration::minutes(11))
        .await
        .unwrap();
    assert_eq!(evidence_counts(&app.state).await, (2, 1, 0, 2));
    assert!(
        subscriptions::use_token(&app.state, &resent, true, now + Duration::minutes(11))
            .await
            .is_err()
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_queue WHERE sent_at IS NULL AND cancelled=FALSE",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn delayed_delivery_starts_a_fresh_cooldown() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "delayed@example.test";
    common::request_subscription(&app.state, email, now - Duration::hours(2))
        .await
        .unwrap();
    let original = app.confirmation_token(email).await;
    mark_delivered(&app.state, now).await;
    common::request_subscription(&app.state, email, now + Duration::seconds(599))
        .await
        .unwrap();
    assert_eq!(evidence_counts(&app.state).await, (1, 1, 1, 1));
    common::request_subscription(&app.state, email, now + Duration::minutes(10))
        .await
        .unwrap();
    assert_eq!(evidence_counts(&app.state).await, (2, 1, 2, 2));
    subscriptions::use_token(&app.state, &original, true, now + Duration::minutes(11))
        .await
        .unwrap();
}

#[tokio::test]
async fn retention_keeps_a_renewed_link_even_when_the_original_pending_request_is_old() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "retained@example.test";
    let mut config = (*app.state.config).clone();
    config.privacy.as_mut().unwrap().retention.pending_days = 1;
    let state = Backend::new(app.state.pool.clone(), config);
    common::request_subscription(&state, email, now - Duration::hours(25))
        .await
        .unwrap();
    mark_delivered(&state, now - Duration::hours(25)).await;
    common::request_subscription(&state, email, now - Duration::hours(2))
        .await
        .unwrap();
    let renewed = app.confirmation_token(email).await;
    assert_eq!(evidence_counts(&state).await, (2, 1, 2, 2));
    privacy::maintenance(&state, now).await.unwrap();
    let requested_at: i64 = sqlx::query_scalar("SELECT requested_at FROM subscription_consents")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(requested_at, (now - Duration::hours(25)).unix_timestamp());
    subscriptions::use_token(&state, &renewed, true, now)
        .await
        .unwrap();
}

#[tokio::test]
async fn rolling_hourly_limit_survives_restart_and_repeated_clicks_do_not_extend_it() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "limited@example.test";
    for minutes in [0, 10, 20] {
        let at = now + Duration::minutes(minutes);
        common::request_subscription(&app.state, email, at)
            .await
            .unwrap();
        mark_delivered(&app.state, at).await;
    }
    assert_eq!(evidence_counts(&app.state).await, (3, 1, 3, 3));
    let restarted = Backend::new(app.state.pool.clone(), (*app.state.config).clone());
    for seconds in [1800, 2400, 3599] {
        common::request_subscription(
            &restarted,
            " LIMITED@example.TEST ",
            now + Duration::seconds(seconds),
        )
        .await
        .unwrap();
        assert_eq!(evidence_counts(&restarted).await, (3, 1, 3, 3));
    }
    common::request_subscription(&restarted, email, now + Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(evidence_counts(&restarted).await, (4, 1, 4, 4));
}

#[tokio::test]
async fn changed_consent_gets_a_new_snapshot_without_bypassing_the_cooldown() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "policy@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let original = app.confirmation_token(email).await;
    mark_delivered(&app.state, now).await;
    let mut config = (*app.state.config).clone();
    config.privacy.as_mut().unwrap().version = "updated-policy".into();
    let updated = Backend::new(app.state.pool.clone(), config);
    common::request_subscription(&updated, email, now + Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(evidence_counts(&updated).await, (1, 1, 1, 1));
    common::request_subscription(&updated, email, now + Duration::minutes(10))
        .await
        .unwrap();
    assert_eq!(evidence_counts(&updated).await, (2, 2, 1, 2));
    let consent_rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT notice_fingerprint,superseded_at FROM subscription_consents ORDER BY id",
    )
    .fetch_all(&updated.pool)
    .await
    .unwrap();
    assert_eq!(consent_rows[0].0, common::consent(&app.state).fingerprint);
    assert_eq!(consent_rows[1].0, common::consent(&updated).fingerprint);
    assert_eq!(
        consent_rows[0].1,
        Some((now + Duration::minutes(10)).unix_timestamp())
    );
    assert_eq!(consent_rows[1].1, None);
    assert!(
        subscriptions::use_token(&updated, &original, true, now + Duration::minutes(11))
            .await
            .is_err()
    );
    let replacement = app.confirmation_token(email).await;
    subscriptions::use_token(&updated, &replacement, true, now + Duration::minutes(11))
        .await
        .unwrap();
}

#[tokio::test]
async fn address_limit_and_confirmed_subscription_return_the_same_accepted_response() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "private@example.test";
    let mut original = String::new();
    for minutes in [30, 20, 10] {
        let at = now - Duration::minutes(minutes);
        common::request_subscription(&app.state, email, at)
            .await
            .unwrap();
        if original.is_empty() {
            original = app.confirmation_token(email).await;
        }
        mark_delivered(&app.state, at).await;
    }
    for confirmed in [false, true] {
        if confirmed {
            subscriptions::use_token(&app.state, &original, true, now)
                .await
                .unwrap();
        }
        assert_eq!(
            app.call(
                "POST",
                "/api/v1/subscriptions",
                Some(json!({"email":email})),
                false
            )
            .await
            .status(),
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(evidence_counts(&app.state).await, (3, 1, 0, 3));
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/subscriptions",
            Some(json!({"email":"other@example.test"})),
            false
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(evidence_counts(&app.state).await, (4, 2, 1, 4));
}

#[tokio::test]
async fn duplicate_submissions_keep_the_twenty_requests_per_hour_ip_limit() {
    let app = App::new().await;
    for attempt in 0..21 {
        let response = app
            .call(
                "POST",
                "/api/v1/subscriptions",
                Some(json!({"email":"ip-limit@example.test"})),
                false,
            )
            .await;
        assert_eq!(
            response.status(),
            if attempt < 20 {
                StatusCode::ACCEPTED
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    assert_eq!(evidence_counts(&app.state).await, (1, 1, 1, 1));
}
