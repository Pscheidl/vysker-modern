#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::App;
use obecni_web::{backend::subscriptions, catalog::SubscriptionPreferences};
use serde_json::json;
use time::{Duration, OffsetDateTime};

async fn preferences(app: &App) -> (bool, Vec<i64>, bool, bool) {
    sqlx::query_as("SELECT all_notice_categories,notice_category_ids,uncategorized_notices,documents FROM subscribers")
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
    assert_eq!(preferences(&app).await, (true, vec![], true, true));
    let token = app.confirmation_token("selection@example.test").await;
    subscriptions::use_token(&app.state, &token, true, now + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(
        preferences(&app).await,
        (false, vec![category], false, false)
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
        (false, vec![category], false, false)
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
    };
    subscriptions::request_subscription_with_preferences(
        &app.state,
        email,
        &common::consent(&app.state),
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
    assert_eq!(preferences(&app).await, (false, vec![], false, true));
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
                        "uncategorized_notices":false, "documents":false
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
