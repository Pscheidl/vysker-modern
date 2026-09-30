#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, body_json};
use http_body_util::BodyExt;
use obecni_web::backend::{auth, subscriptions};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const PASSWORD: &str = "Testovaci-dlouhe-heslo-123456!";
const BASE: &str = "/api/v1/admin/subscribers";

async fn subscriber(app: &App, email: &str, confirmed: bool) -> i64 {
    let now = OffsetDateTime::now_utc() - Duration::minutes(1);
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    if confirmed {
        let token = app.confirmation_token(email).await;
        subscriptions::use_token(&app.state, &token, true, now)
            .await
            .unwrap();
    }
    sqlx::query_scalar("SELECT id FROM subscribers WHERE email=$1")
        .bind(email)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn request(
    app: &App,
    method: &str,
    path: &str,
    csrf: Option<&str>,
    origin: Option<&str>,
    body: Value,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", &app.cookie)
        .header("content-type", "application/json");
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    app.router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn list(app: &App, query: &str) -> Value {
    let response = app.call("GET", &format!("{BASE}{query}"), None, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    body_json(response).await
}

fn ids(value: &Value) -> Vec<i64> {
    value["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn administration_requires_current_admin_and_public_routes_do_not_leak_identity() {
    let app = App::new().await;
    let email = "private-subscriber@example.test";
    let id = subscriber(&app, email, false).await;
    for (method, path) in [
        ("GET", BASE.into()),
        ("GET", format!("{BASE}/{id}/export")),
        ("GET", format!("{BASE}/{id}/export?download=true")),
        ("POST", format!("{BASE}/{id}/withdraw")),
        ("DELETE", format!("{BASE}/{id}")),
    ] {
        let response = app
            .call(
                method,
                &path,
                Some(json!({"current_password":PASSWORD})),
                false,
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&bytes).contains(email));
    }
    for path in [
        "/api/v1/subscribers".into(),
        format!("/api/v1/subscribers/{id}/export"),
        "/api/v1/search?q=private-subscriber".into(),
        "/api/v1/privacy".into(),
    ] {
        let response = app.call("GET", &path, None, false).await;
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&bytes).contains(email));
    }
    // The public opt-in endpoint remains an acknowledgement, never an admin import.
    let response = app
        .call(
            "POST",
            "/api/v1/subscriptions",
            Some(json!({"email":email})),
            false,
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let verified: Option<i64> =
        sqlx::query_scalar("SELECT verified_at FROM subscribers WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(verified.is_none());
    sqlx::query("UPDATE administrators SET active=FALSE WHERE id=$1")
        .bind(app.admin_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", BASE, None, true).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call("GET", &format!("{BASE}/{id}/export"), None, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{BASE}/{id}"),
            Some(json!({"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn destructive_actions_require_csrf_same_origin_and_own_current_password() {
    let app = App::new().await;
    let id = subscriber(&app, "protected@example.test", false).await;
    let before = body_json(
        app.call("GET", &format!("{BASE}/{id}/export"), None, true)
            .await,
    )
    .await;
    for (method, path) in [
        ("POST", format!("{BASE}/{id}/withdraw")),
        ("DELETE", format!("{BASE}/{id}")),
    ] {
        let body = json!({"current_password":PASSWORD});
        for csrf in [None, Some("wrong")] {
            assert_eq!(
                request(&app, method, &path, csrf, None, body.clone())
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            request(
                &app,
                method,
                &path,
                Some(&app.csrf),
                Some("https://foreign.example"),
                body
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.call(
                method,
                &path,
                Some(json!({"current_password":"wrong"})),
                true
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.call(method, &path, Some(json!({})), true)
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            app.call(
                method,
                &path,
                Some(json!({"current_password":PASSWORD,"activate":true})),
                true
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let after = body_json(
        app.call("GET", &format!("{BASE}/{id}/export"), None, true)
            .await,
    )
    .await;
    assert_eq!(before["subscriber"], after["subscriber"]);
    assert_eq!(before["consents"], after["consents"]);
    assert_eq!(before["mail_history"], after["mail_history"]);
    let changes: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE operation IN ('subscriber_withdrawn','subscriber_erased')")
        .fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(changes, 0);
    // Knowing another administrator's password cannot authorize this session.
    auth::create_admin(
        &app.state,
        "other-admin@example.test",
        "Another-administrator-password!".into(),
    )
    .await
    .unwrap();
    assert_eq!(
        app.call(
            "POST",
            &format!("{BASE}/{id}/withdraw"),
            Some(json!({"current_password":"Another-administrator-password!"})),
            true
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn search_is_literal_and_normalized_with_proven_status_and_total_on_every_page() {
    let app = App::new().await;
    let active = subscriber(&app, "alice@example.test", true).await;
    let pending = subscriber(&app, "pending@example.test", false).await;
    let mut extra = Vec::new();
    for email in [
        "percent%box@example.test",
        "under_score@example.test",
        "back\\slash@example.test",
        "quote'box@example.test",
        "legacy@example.test",
        "withdrawn@example.test",
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO subscribers(email,retention_started_at) VALUES ($1,1) RETURNING id",
        )
        .bind(email)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        extra.push(id);
    }
    sqlx::query("UPDATE subscribers SET verified_at=1 WHERE id=$1")
        .bind(extra[4])
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE subscribers SET unsubscribed_at=2 WHERE id=$1")
        .bind(extra[5])
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(ids(&list(&app, "?status=active").await), vec![active]);
    let waiting = list(&app, "?status=pending").await;
    assert_eq!(waiting["total"], 6);
    assert!(ids(&waiting).contains(&pending));
    assert!(ids(&waiting).contains(&extra[4]));
    assert_eq!(
        ids(&list(&app, "?status=unsubscribed").await),
        vec![extra[5]]
    );
    assert_eq!(
        ids(&list(&app, "?q=%20ALICE%40EXAMPLE.TEST%20&status=active").await),
        vec![active]
    );
    assert_eq!(list(&app, "?q=ALICE&status=pending").await["total"], 0);
    for (needle, id) in [
        ("%25", extra[0]),
        ("_", extra[1]),
        ("%5C", extra[2]),
        ("%27", extra[3]),
    ] {
        assert_eq!(ids(&list(&app, &format!("?q={needle}")).await), vec![id]);
    }
    assert_eq!(list(&app, "?q=%27%20OR%201%3D1").await["total"], 0);
    let all = list(&app, "").await;
    assert_eq!(all["total"], 8);
    let mut paged = Vec::new();
    for offset in [0, 2, 4, 6, 8] {
        let page = list(&app, &format!("?limit=2&offset={offset}")).await;
        assert_eq!(page["total"], 8);
        assert_eq!(page["limit"], 2);
        assert_eq!(page["offset"], offset);
        paged.extend(ids(&page));
    }
    assert_eq!(paged, ids(&all));
    assert!(ids(&list(&app, "?offset=999").await).is_empty());
    for query in [
        "?status=bogus",
        "?limit=0",
        "?limit=101",
        "?offset=-1",
        "?q=%0Ainvalid",
        "?unknown=true",
    ] {
        assert_eq!(
            app.call("GET", &format!("{BASE}{query}"), None, true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn export_preserves_exact_old_snapshots_and_retained_smtp_metadata_without_secrets() {
    let app = App::new().await;
    let id = subscriber(&app, "evidence@example.test", false).await;
    let secret = app.confirmation_token("evidence@example.test").await;
    let privacy = "{\n  \"version\": \"historical\", \"removed_field\": [1, 2]\n}";
    let wording = "Původní souhlas\nse zvláštními znaky <>& a původním účelem.";
    sqlx::query("INSERT INTO consent_notices(fingerprint,version,consent_text,privacy_json) VALUES ('old-proof','old-version',$1,$2)")
        .bind(wording).bind(privacy).execute(&app.state.pool).await.unwrap();
    let old: i64 = sqlx::query_scalar("INSERT INTO subscription_consents(subscriber_id,notice_fingerprint,requested_at,confirmed_at,withdrawn_at,superseded_at) VALUES ($1,'old-proof',10,20,30,40) RETURNING id")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("UPDATE mail_queue SET attempts=3,lock_token='private-smtp-lease',locked_until=99 WHERE subscriber_id=$1")
        .bind(id).execute(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO mail_queue(subscriber_id,consent_id,purpose,subject,body,attempts,next_attempt_at,created_at,sent_at) VALUES ($1,$2,'document','private-subject','private-body',2,10,10,20)")
        .bind(id).bind(old).execute(&app.state.pool).await.unwrap();
    let path = format!("{BASE}/{id}/export?download=true");
    let response = app.call("GET", &path, None, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        format!("attachment; filename=\"odberatel-{id}.json\"")
    );
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let evidence = body_json(response).await;
    let snapshot = evidence["consents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == old)
        .unwrap();
    assert_eq!(snapshot["privacy_json"], privacy);
    assert_eq!(snapshot["consent_text"], wording);
    assert_eq!(snapshot["version"], "old-version");
    assert_eq!(snapshot["confirmed_at"], 20);
    assert_eq!(snapshot["withdrawn_at"], 30);
    assert_eq!(snapshot["superseded_at"], 40);
    assert_eq!(evidence["mail_history"].as_array().unwrap().len(), 2);
    assert_eq!(evidence["mail_history"][0]["attempts"], 3);
    assert_eq!(evidence["mail_history"][1]["sent_at"], 20);
    assert_eq!(evidence["mail_history"][1]["consent_id"], old);
    assert!(!evidence["exported_at"].as_str().unwrap().is_empty());
    let serialized = evidence.to_string();
    for forbidden in [
        secret.clone(),
        auth::hash(&secret),
        "private-smtp-lease".into(),
        "private-subject".into(),
        "private-body".into(),
        app.csrf.clone(),
        "password_hash".into(),
        "token_hash".into(),
        "lock_token".into(),
        "\"body\"".into(),
    ] {
        assert!(
            !serialized.contains(&forbidden),
            "Export leaked {forbidden}"
        );
    }
    let audit: (i64, i64, Option<String>, Option<String>) = sqlx::query_as("SELECT actor_id,entity_id,details,ip_address FROM audit_log WHERE operation='subscriber_exported'")
        .fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(audit, (app.admin_id, id, None, None));
    let response = app
        .call("GET", &format!("{BASE}/{id}/export"), None, true)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("content-disposition"));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE operation='subscriber_exported'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn withdrawal_revokes_tokens_cancels_all_pending_mail_and_preserves_timestamp_evidence() {
    let app = App::new().await;
    let id = subscriber(&app, "withdraw@example.test", true).await;
    let publication = app.notice(json!({})).await;
    assert!(app.publish(publication).await.status().is_success());
    let body: String = sqlx::query_scalar(
        "SELECT body FROM mail_queue WHERE subscriber_id=$1 AND purpose='document'",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    let unsubscribe = common::token_from_body(&body);
    let start = OffsetDateTime::now_utc().unix_timestamp();
    let path = format!("{BASE}/{id}/withdraw");
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(json!({"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let row: (i64, i64, String, Option<String>) = sqlx::query_as("SELECT s.unsubscribed_at,c.withdrawn_at,a.occurred_at,a.details FROM subscribers s JOIN subscription_consents c ON c.subscriber_id=s.id JOIN audit_log a ON a.entity_id=s.id AND a.entity_type='subscriber' AND a.operation='subscriber_withdrawn' WHERE s.id=$1")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert!(row.0 >= start);
    assert_eq!(row.0, row.1);
    assert_eq!(
        OffsetDateTime::parse(&row.2, &time::format_description::well_known::Rfc3339)
            .unwrap()
            .unix_timestamp(),
        row.0
    );
    assert!(row.3.is_none());
    assert_eq!(
        subscriptions::use_token(&app.state, &unsubscribe, false, OffsetDateTime::now_utc())
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscription_tokens WHERE subscriber_id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    let mail: Vec<(bool, Option<String>)> = sqlx::query_as(
        "SELECT cancelled,body FROM mail_queue WHERE subscriber_id=$1 AND sent_at IS NULL",
    )
    .bind(id)
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert!(!mail.is_empty());
    assert!(mail.iter().all(|row| row.0 && row.1.is_none()));
    let second = app.notice(json!({"title":"After withdrawal"})).await;
    assert!(app.publish(second).await.status().is_success());
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_queue WHERE subscriber_id=$1 AND sent_at IS NULL AND cancelled=FALSE")
        .bind(id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(pending, 0);
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(json!({"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let timestamp: i64 = sqlx::query_scalar("SELECT unsubscribed_at FROM subscribers WHERE id=$1")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(timestamp, row.0);
    assert_eq!(ids(&list(&app, "?status=unsubscribed").await), vec![id]);
}

#[tokio::test]
async fn pending_withdrawal_revokes_confirmation_and_never_activates_subscription() {
    let app = App::new().await;
    let id = subscriber(&app, "pending-withdrawal@example.test", false).await;
    let token = app
        .confirmation_token("pending-withdrawal@example.test")
        .await;
    assert_eq!(
        app.call(
            "POST",
            &format!("{BASE}/{id}/withdraw"),
            Some(json!({"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        subscriptions::use_token(&app.state, &token, true, OffsetDateTime::now_utc())
            .await
            .is_err()
    );
    let export = body_json(
        app.call("GET", &format!("{BASE}/{id}/export"), None, true)
            .await,
    )
    .await;
    assert!(export["subscriber"]["verified_at"].is_null());
    assert!(export["consents"][0]["confirmed_at"].is_null());
    assert!(export["consents"][0]["withdrawn_at"].is_i64());
}

#[tokio::test]
async fn erasure_cascades_identity_consent_tokens_and_mail_with_numeric_audit_only() {
    let app = App::new().await;
    let email = "erase@example.test";
    let id = subscriber(&app, email, false).await;
    let survivor = subscriber(&app, "survivor@example.test", false).await;
    let secret = app.confirmation_token(email).await;
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{BASE}/{id}"),
            Some(json!({"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    for (table, key) in [
        ("subscribers", "id"),
        ("subscription_consents", "subscriber_id"),
        ("subscription_tokens", "subscriber_id"),
        ("mail_queue", "subscriber_id"),
    ] {
        let count: i64 = sqlx::QueryBuilder::<sqlx::Postgres>::new(format!(
            "SELECT count(*) FROM {table} WHERE {key}=$1"
        ))
        .build_query_scalar()
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "{table}");
        let count: i64 = sqlx::QueryBuilder::<sqlx::Postgres>::new(format!(
            "SELECT count(*) FROM {table} WHERE {key}=$1"
        ))
        .build_query_scalar()
        .bind(survivor)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert!(count > 0, "Surviving subscriber lost {table}");
    }
    let audit: (i64, i64, Option<String>, Option<String>) = sqlx::query_as("SELECT actor_id,entity_id,details,ip_address FROM audit_log WHERE operation='subscriber_erased'")
        .fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(audit, (app.admin_id, id, None, None));
    let audit_json: String = sqlx::query_scalar(
        "SELECT json_agg(a)::text FROM audit_log a WHERE entity_type='subscriber' AND entity_id=$1",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(!audit_json.contains(email));
    assert!(!audit_json.contains(&secret));
    assert!(!audit_json.contains(&auth::hash(&secret)));
    assert!(
        subscriptions::use_token(&app.state, &secret, true, OffsetDateTime::now_utc())
            .await
            .is_err()
    );
    assert_eq!(
        app.call("GET", &format!("{BASE}/{id}/export"), None, true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{BASE}/{id}"),
            Some(json!({"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn live_smtp_lease_blocks_erasure_even_after_withdrawal_until_lease_expires() {
    let app = App::new().await;
    let id = subscriber(&app, "leased@example.test", false).await;
    let lease_until = OffsetDateTime::now_utc().unix_timestamp() + 300;
    sqlx::query(
        "UPDATE mail_queue SET locked_until=$1,lock_token='active-lease' WHERE subscriber_id=$2",
    )
    .bind(lease_until)
    .bind(id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    let path = format!("{BASE}/{id}");
    for withdrawn in [false, true] {
        if withdrawn {
            assert_eq!(
                app.call(
                    "POST",
                    &format!("{path}/withdraw"),
                    Some(json!({"current_password":PASSWORD})),
                    true
                )
                .await
                .status(),
                StatusCode::NO_CONTENT
            );
        }
        let response = app
            .call(
                "DELETE",
                &path,
                Some(json!({"current_password":PASSWORD})),
                true,
            )
            .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let retry = response.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
        assert!((1..=300).contains(&retry));
        assert_eq!(body_json(response).await["retry_after_seconds"], retry);
        let row: (bool, i64, Option<String>) = sqlx::query_as(
            "SELECT cancelled,locked_until,lock_token FROM mail_queue WHERE subscriber_id=$1",
        )
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(row, (withdrawn, lease_until, Some("active-lease".into())));
    }
    let erased: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE operation='subscriber_erased'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(erased, 0);
    sqlx::query("UPDATE mail_queue SET locked_until=$1 WHERE subscriber_id=$2")
        .bind(OffsetDateTime::now_utc().unix_timestamp() - 1)
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call(
            "DELETE",
            &path,
            Some(json!({"current_password":PASSWORD})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn audit_failure_rolls_back_withdrawal_and_erasure_completely() {
    let app = App::new().await;
    let id = subscriber(&app, "atomic@example.test", false).await;
    // Inject a database failure after all destructive writes but before commit.
    sqlx::raw_sql("CREATE FUNCTION reject_subscriber_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.operation IN ('subscriber_withdrawn','subscriber_erased') THEN RAISE EXCEPTION 'test audit unavailable'; END IF; RETURN NEW; END; $$; CREATE TRIGGER subscriber_audit_failure BEFORE INSERT ON audit_log FOR EACH ROW EXECUTE FUNCTION reject_subscriber_audit();")
        .execute(&app.state.pool).await.unwrap();
    let before = body_json(
        app.call("GET", &format!("{BASE}/{id}/export"), None, true)
            .await,
    )
    .await;
    for (method, path) in [
        ("POST", format!("{BASE}/{id}/withdraw")),
        ("DELETE", format!("{BASE}/{id}")),
    ] {
        assert_eq!(
            app.call(
                method,
                &path,
                Some(json!({"current_password":PASSWORD})),
                true
            )
            .await
            .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        let after = body_json(
            app.call("GET", &format!("{BASE}/{id}/export"), None, true)
                .await,
        )
        .await;
        assert_eq!(before["subscriber"], after["subscriber"]);
        assert_eq!(before["consents"], after["consents"]);
        assert_eq!(before["mail_history"], after["mail_history"]);
        let token_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM subscription_tokens WHERE subscriber_id=$1")
                .bind(id)
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
        assert_eq!(token_count, 1);
    }
}
