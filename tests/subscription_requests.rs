#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::App;
use obecni_web::{
    backend::{auth, subscriptions},
    catalog::SubscriptionPreferences,
};
use serde_json::json;
use time::{Duration, OffsetDateTime};

async fn preferences(app: &App) -> (bool, Vec<i64>, bool, bool, bool) {
    sqlx::query_as("SELECT all_notice_categories,notice_category_ids,uncategorized_notices,documents,events FROM subscribers")
        .fetch_one(&app.state.pool).await.unwrap()
}

#[tokio::test]
async fn selected_topics_activate_only_with_the_exact_confirmed_request() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let category: i64 = sqlx::query_scalar("SELECT id FROM categories ORDER BY id LIMIT 1")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let selection = SubscriptionPreferences {
        all_notice_categories: false,
        notice_category_ids: vec![category, category],
        uncategorized_notices: false,
        documents: false,
        events: false,
    };
    let response = app
        .call(
            "POST",
            "/api/v1/subscriptions",
            Some(json!({
                "email": "selection@example.test", "preferences": selection
            })),
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(preferences(&app).await, (true, vec![], true, true, true));
    let token = app.confirmation_token("selection@example.test").await;
    subscriptions::use_token(&app.state, &token, true, now + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(
        preferences(&app).await,
        (false, vec![category], false, false, false)
    );

    // Knowing an address is not authorization to broaden somebody else's mail.
    let response = app
        .call(
            "POST",
            "/api/v1/subscriptions",
            Some(json!({
                "email": "selection@example.test"
            })),
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        preferences(&app).await,
        (false, vec![category], false, false, false)
    );
}

#[tokio::test]
async fn changed_pending_selection_invalidates_old_confirmation_after_cooldown() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "changed@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let original = app.confirmation_token(email).await;
    sqlx::query("UPDATE mail_queue SET sent_at=$1,body=NULL")
        .bind(now.unix_timestamp())
        .execute(&app.state.pool)
        .await
        .unwrap();
    let selection = SubscriptionPreferences {
        all_notice_categories: false,
        notice_category_ids: vec![],
        uncategorized_notices: false,
        documents: true,
        events: false,
    };
    subscriptions::request_subscription_with_preferences(
        &app.state,
        email,
        &common::consent(&app.state).await,
        &selection,
        now + Duration::minutes(10),
    )
    .await
    .unwrap();
    let current = app.confirmation_token(email).await;
    assert!(
        subscriptions::use_token(&app.state, &original, true, now + Duration::minutes(11))
            .await
            .is_err()
    );
    subscriptions::use_token(&app.state, &current, true, now + Duration::minutes(11))
        .await
        .unwrap();
    assert_eq!(preferences(&app).await, (false, vec![], false, true, false));
}

#[tokio::test]
async fn empty_or_unknown_topics_are_rejected_without_creating_subscription() {
    let app = App::new().await;
    for ids in [vec![], vec![999999]] {
        let response = app
            .call(
                "POST",
                "/api/v1/subscriptions",
                Some(json!({
                    "email":"invalid@example.test", "preferences": {
                        "all_notice_categories":false, "notice_category_ids":ids,
                        "uncategorized_notices":false, "documents":false, "events":false
                    }
                })),
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
}

#[tokio::test]
async fn calendar_only_request_is_valid_and_defaults_on_when_omitted() {
    let app = App::new().await;
    let response = app
        .call(
            "POST",
            "/api/v1/subscriptions",
            Some(json!({
                "email": "calendar-only@example.test", "preferences": {
                    "all_notice_categories": false, "notice_category_ids": [],
                    "uncategorized_notices": false, "documents": false
                }
            })),
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: String =
        sqlx::query_scalar("SELECT body FROM mail_queue WHERE purpose='verification'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(body.contains("Vybraný odběr: Kalendář akcí."));
    let token = app.confirmation_token("calendar-only@example.test").await;
    subscriptions::use_token(&app.state, &token, true, OffsetDateTime::now_utc())
        .await
        .unwrap();
    assert_eq!(preferences(&app).await, (false, vec![], false, false, true));
}

#[tokio::test]
async fn changing_only_events_in_a_pending_request_requires_a_new_confirmation() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "changed-calendar@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let original = app.confirmation_token(email).await;
    sqlx::query("UPDATE mail_queue SET sent_at=$1,body=NULL")
        .bind(now.unix_timestamp())
        .execute(&app.state.pool)
        .await
        .unwrap();
    subscriptions::request_subscription_with_preferences(
        &app.state,
        email,
        &common::consent(&app.state).await,
        &SubscriptionPreferences {
            events: false,
            ..SubscriptionPreferences::default()
        },
        now + Duration::minutes(10),
    )
    .await
    .unwrap();
    let current = app.confirmation_token(email).await;
    assert!(
        subscriptions::use_token(&app.state, &original, true, now + Duration::minutes(11))
            .await
            .is_err()
    );
    subscriptions::use_token(&app.state, &current, true, now + Duration::minutes(11))
        .await
        .unwrap();
    assert_eq!(preferences(&app).await, (true, vec![], true, true, false));
    let snapshots: Vec<Option<bool>> =
        sqlx::query_scalar("SELECT events FROM subscription_consents ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(snapshots, vec![Some(true), Some(false)]);
}

#[tokio::test]
async fn old_pending_consent_enables_events_without_rewriting_its_evidence() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, "current-request@example.test", now)
        .await
        .unwrap();
    let subscriber: i64 = sqlx::query_scalar("INSERT INTO subscribers(email,retention_started_at) VALUES ('legacy-pending@example.test',$1) RETURNING id")
        .bind(now.unix_timestamp()).fetch_one(&app.state.pool).await.unwrap();
    let consent: i64 = sqlx::query_scalar("INSERT INTO subscription_consents(subscriber_id,notice_fingerprint,requested_at,events) SELECT $1,notice_fingerprint,$2,NULL FROM subscription_consents LIMIT 1 RETURNING id")
        .bind(subscriber).bind(now.unix_timestamp()).fetch_one(&app.state.pool).await.unwrap();
    let token = auth::token();
    sqlx::query("INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id) VALUES ($1,$2,'verification',$3,$4)")
        .bind(auth::hash(&token)).bind(subscriber).bind(now.unix_timestamp()+86400).bind(consent)
        .execute(&app.state.pool).await.unwrap();
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
    subscriptions::use_token(&app.state, &token, true, now)
        .await
        .unwrap();
    let state: (bool, Option<bool>) = sqlx::query_as("SELECT s.events,c.events FROM subscribers s JOIN subscription_consents c ON c.subscriber_id=s.id WHERE s.id=$1")
        .bind(subscriber).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(state, (true, None));
}
