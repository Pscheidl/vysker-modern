#![cfg(feature = "ssr")]
mod common;

use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::backend::{Backend, mail, subscriptions};
use serde_json::json;
use time::OffsetDateTime;

async fn subscriber(app: &App, email: &str) -> i64 {
    let now = OffsetDateTime::now_utc();
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let token = app.confirmation_token(email).await;
    subscriptions::use_token(&app.state, &token, true, now)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT id FROM subscribers WHERE email=$1")
        .bind(email)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn preferences(
    app: &App,
    subscriber: i64,
    all_categories: bool,
    categories: &[i64],
    uncategorized: bool,
    documents: bool,
) {
    sqlx::query("UPDATE subscribers SET all_notice_categories=$2,notice_category_ids=$3,uncategorized_notices=$4,documents=$5 WHERE id=$1")
        .bind(subscriber).bind(all_categories).bind(categories).bind(uncategorized).bind(documents)
        .execute(&app.state.pool).await.unwrap();
}

async fn category(app: &App, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO categories(name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn notice(app: &App, category: Option<i64>) -> i64 {
    let id = app.notice(json!({"category_id":category})).await;
    assert_eq!(app.publish(id).await.status(), StatusCode::NO_CONTENT);
    id
}

async fn document(app: &App) -> i64 {
    let response = app
        .call(
            "POST",
            "/api/v1/admin/documents",
            Some(json!({"title":"Obecný dokument","description":"Informace"})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = body_json(response).await["id"].as_i64().unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/admin/documents/{id}/publish"),
            None,
            true,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    id
}

async fn recipients(app: &App, notice: Option<i64>, document: Option<i64>) -> Vec<i64> {
    sqlx::query_scalar("SELECT subscriber_id FROM publication_outbox WHERE notice_id IS NOT DISTINCT FROM $1 AND document_id IS NOT DISTINCT FROM $2 ORDER BY subscriber_id")
        .bind(notice).bind(document).fetch_all(&app.state.pool).await.unwrap()
}

#[tokio::test]
async fn native_publication_snapshots_default_categories_uncategorized_and_documents() {
    let app = App::new().await;
    let chosen_category = category(&app, "Vybraná kategorie").await;
    let default = subscriber(&app, "everything@example.test").await;
    let selected = subscriber(&app, "selected@example.test").await;
    let documents = subscriber(&app, "documents@example.test").await;
    let uncategorized = subscriber(&app, "uncategorized@example.test").await;
    let named = subscriber(&app, "named@example.test").await;
    preferences(&app, selected, false, &[chosen_category], false, false).await;
    preferences(&app, documents, false, &[], false, true).await;
    preferences(&app, uncategorized, false, &[], true, false).await;
    preferences(&app, named, true, &[], false, false).await;

    // New categories join the all-categories choice automatically.
    let later_category = category(&app, "Nová kategorie").await;
    let chosen = notice(&app, Some(chosen_category)).await;
    let other = notice(&app, Some(later_category)).await;
    let unclassified = notice(&app, None).await;
    let general = document(&app).await;
    assert_eq!(
        recipients(&app, Some(chosen), None).await,
        vec![default, selected, named]
    );
    assert_eq!(
        recipients(&app, Some(other), None).await,
        vec![default, named]
    );
    assert_eq!(
        recipients(&app, Some(unclassified), None).await,
        vec![default, uncategorized]
    );
    assert_eq!(
        recipients(&app, None, Some(general)).await,
        vec![default, documents]
    );

    assert!(
        mail::prepare_publications(&app.state, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    let bodies: Vec<String> =
        sqlx::query_scalar("SELECT body FROM mail_queue WHERE purpose='document'")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(bodies.len(), 9);
    for body in bodies {
        let token = common::token_from_body(&body);
        assert!(body.contains(&format!("/odber/odhlasit?token={token}")));
        assert!(body.contains(&format!("/odber/nastaveni?token={token}")));
    }
}

#[tokio::test]
async fn delayed_outbox_rechecks_preferences_without_adding_new_recipients() {
    let app = App::new().await;
    let original = subscriber(&app, "original@example.test").await;
    let excluded = subscriber(&app, "excluded@example.test").await;
    preferences(&app, excluded, false, &[], false, true).await;
    let id = notice(&app, None).await;
    assert_eq!(recipients(&app, Some(id), None).await, vec![original]);

    preferences(&app, original, false, &[], false, true).await;
    preferences(&app, excluded, true, &[], true, true).await;
    assert!(
        mail::prepare_publications(&app.state, OffsetDateTime::now_utc())
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_queue WHERE purpose='document'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        0
    );
    assert!(recipients(&app, Some(id), None).await.is_empty());

    let next = notice(&app, None).await;
    assert_eq!(recipients(&app, Some(next), None).await, vec![excluded]);
}

#[tokio::test]
async fn queued_notice_and_document_recheck_preferences_before_smtp() {
    let app = App::new().await;
    let recipient = subscriber(&app, "queued@example.test").await;
    let category = category(&app, "Vyhlášky").await;
    notice(&app, Some(category)).await;
    document(&app).await;
    let now = OffsetDateTime::now_utc();
    assert!(mail::prepare_publications(&app.state, now).await.unwrap());
    // Keep uncategorized notices enabled while dropping both queued subjects.
    preferences(&app, recipient, false, &[], true, false).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.state.config).clone();
    config.smtp_port = listener.local_addr().unwrap().port();
    drop(listener);
    let state = Backend::new(app.state.pool.clone(), config);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, now).await.unwrap());
    assert!(mail::deliver_one(&state, &smtp, now).await.unwrap());
    assert!(!mail::deliver_one(&state, &smtp, now).await.unwrap());
    let messages: Vec<(bool, Option<String>, Option<i64>, i64)> = sqlx::query_as(
        "SELECT cancelled,body,sent_at,attempts FROM mail_queue WHERE purpose='document' ORDER BY id")
        .fetch_all(&state.pool).await.unwrap();
    assert_eq!(messages, vec![(true, None, None, 1), (true, None, None, 1)]);
}

#[tokio::test]
async fn queued_document_is_still_delivered_when_only_notice_preferences_change() {
    let app = App::new().await;
    let recipient = subscriber(&app, "document-queue@example.test").await;
    document(&app).await;
    let now = OffsetDateTime::now_utc();
    assert!(mail::prepare_publications(&app.state, now).await.unwrap());
    preferences(&app, recipient, false, &[], false, true).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*app.state.config).clone();
    config.smtp_port = listener.local_addr().unwrap().port();
    drop(listener);
    let state = Backend::new(app.state.pool.clone(), config);
    let smtp = mail::transport(&state).unwrap();
    assert!(mail::deliver_one(&state, &smtp, now).await.unwrap());
    let message: (bool, bool, i64) = sqlx::query_as(
        "SELECT cancelled,body IS NOT NULL,attempts FROM mail_queue WHERE purpose='document'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    // The matching message reached SMTP and remains available for its retry.
    assert_eq!(message, (false, true, 1));
}

#[tokio::test]
#[ignore = "vyžaduje Mailpit, spusťte scripts/test.sh --smtp"]
async fn mailpit_receives_only_matching_publications_and_unfiltered_verification() {
    let app = App::new().await;
    let mut config = (*app.state.config).clone();
    config.smtp_host = std::env::var("TEST_SMTP_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    config.smtp_port = std::env::var("TEST_SMTP_PORT")
        .unwrap_or_else(|_| "1025".into())
        .parse()
        .unwrap();
    let state = Backend::new(app.state.pool.clone(), config);
    let smtp = mail::transport(&state).unwrap();

    let first_category = category(&app, "Mailpit první kategorie").await;
    let second_category = category(&app, "Mailpit druhá kategorie").await;
    let third_category = category(&app, "Mailpit třetí kategorie").await;
    let everything = subscriber(&app, "mailpit-filter-all@example.test").await;
    let selected = subscriber(&app, "mailpit-filter-selected@example.test").await;
    let documents = subscriber(&app, "mailpit-filter-documents@example.test").await;
    let changed = subscriber(&app, "mailpit-filter-changed@example.test").await;
    preferences(
        &app,
        selected,
        false,
        &[first_category, third_category],
        false,
        false,
    )
    .await;
    preferences(&app, documents, false, &[], false, true).await;
    preferences(&app, changed, false, &[second_category], false, false).await;

    let first = notice(&app, Some(first_category)).await;
    let second = notice(&app, Some(second_category)).await;
    let third = notice(&app, Some(third_category)).await;
    let uncategorized = notice(&app, None).await;
    let general = document(&app).await;
    assert_eq!(
        recipients(&app, Some(first), None).await,
        vec![everything, selected]
    );
    assert_eq!(
        recipients(&app, Some(second), None).await,
        vec![everything, changed]
    );
    assert_eq!(
        recipients(&app, Some(third), None).await,
        vec![everything, selected]
    );
    assert_eq!(
        recipients(&app, Some(uncategorized), None).await,
        vec![everything]
    );
    assert_eq!(
        recipients(&app, None, Some(general)).await,
        vec![everything, documents]
    );

    let now = OffsetDateTime::now_utc();
    assert!(mail::prepare_publications(&state, now).await.unwrap());
    // An already queued notice must respect the recipient's latest selection.
    preferences(&app, changed, false, &[], false, true).await;

    common::request_subscription(&state, "mailpit-filter-verification@example.test", now)
        .await
        .unwrap();
    let pending: i64 = sqlx::query_scalar(
        "SELECT id FROM subscribers WHERE email='mailpit-filter-verification@example.test'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    preferences(&app, pending, false, &[], false, false).await;

    for _ in 0..10 {
        assert!(mail::deliver_one(&state, &smtp, now).await.unwrap());
    }
    assert!(!mail::deliver_one(&state, &smtp, now).await.unwrap());

    let delivered: Vec<String> = sqlx::query_scalar(
        "SELECT deduplication_key FROM mail_queue WHERE purpose='document' AND sent_at IS NOT NULL ORDER BY deduplication_key",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap();
    let mut expected = vec![
        format!("notice:{first}:{everything}"),
        format!("notice:{second}:{everything}"),
        format!("notice:{third}:{everything}"),
        format!("notice:{uncategorized}:{everything}"),
        format!("document:{general}:{everything}"),
        format!("notice:{first}:{selected}"),
        format!("notice:{third}:{selected}"),
        format!("document:{general}:{documents}"),
    ];
    expected.sort();
    assert_eq!(
        delivered, expected,
        "Mailpit musí přijmout právě publikace odpovídající výběru odběratelů"
    );
    let cancelled: (bool, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT cancelled,sent_at,body FROM mail_queue WHERE subscriber_id=$1 AND purpose='document'",
    )
    .bind(changed)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(cancelled, (true, None, None));
    let verification: (bool, bool, bool) = sqlx::query_as(
        "SELECT cancelled,sent_at IS NOT NULL,body IS NULL FROM mail_queue WHERE subscriber_id=$1 AND purpose='verification'",
    )
    .bind(pending)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(verification, (false, true, true));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM mail_queue WHERE purpose='document' AND (sent_at IS NULL AND NOT cancelled OR body IS NOT NULL)",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn consent_preference_snapshot_is_immutable_before_and_after_confirmation() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "immutable-selection@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    let selected_category: i64 =
        sqlx::query_scalar("SELECT id FROM categories ORDER BY id LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    for confirmed in [false, true] {
        if confirmed {
            let token = app.confirmation_token(email).await;
            subscriptions::use_token(&app.state, &token, true, now)
                .await
                .unwrap();
        }
        for statement in [
            "UPDATE subscription_consents SET all_notice_categories=FALSE",
            "UPDATE subscription_consents SET uncategorized_notices=FALSE",
            "UPDATE subscription_consents SET documents=FALSE",
        ] {
            let error = sqlx::query(statement)
                .execute(&app.state.pool)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("consent evidence is immutable"));
        }
        let error = sqlx::query("UPDATE subscription_consents SET notice_category_ids=$1")
            .bind(vec![selected_category])
            .execute(&app.state.pool)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("consent evidence is immutable"));
    }
    let snapshot: (bool, Vec<i64>, bool, bool) = sqlx::query_as(
        "SELECT all_notice_categories,notice_category_ids,uncategorized_notices,documents FROM subscription_consents")
        .fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(snapshot, (true, vec![], true, true));
}
