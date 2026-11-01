#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, body_json};
use serde_json::{Value, json};
use tower::ServiceExt;

const FIRST_PASSWORD: &str = "Testovaci-dlouhe-heslo-123456!";
const SECOND_PASSWORD: &str = "Dalsi-velmi-dlouhe-heslo-123456!";

async fn decision(app: &App, cookie: &str, csrf: &str, rental: i64, operation: &str) -> Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/admin/rentals/{rental}/{operation}"))
                .header("cookie", cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn pending(app: &App, asset: i64, offset: i32) -> i64 {
    sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,rental_price,created_at,identity_consent_at,require_meter) VALUES ($1,CURRENT_DATE + $2,CURRENT_DATE + $2,'Test','test@example.test','123456','Test',0,'now','now',FALSE) RETURNING id")
        .bind(asset).bind(offset).fetch_one(&app.state.pool).await.unwrap()
}

async fn request_as(
    app: &App,
    cookie: &str,
    csrf: &str,
    method: &str,
    path: &str,
    value: Option<Value>,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie)
        .header("x-csrf-token", csrf);
    let body = if let Some(value) = value {
        request = request.header("content-type", "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };
    app.router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn basic_account(app: &App) -> (i64, String, String) {
    let account = app.call("POST", "/api/v1/admin/accounts", Some(json!({
        "email": "dalsi@vysker.test", "password": SECOND_PASSWORD, "current_password": FIRST_PASSWORD
    })), true).await;
    assert_eq!(account.status(), StatusCode::CREATED);
    let account_id = body_json(account).await["id"].as_i64().unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({
                "email": "dalsi@vysker.test", "password": SECOND_PASSWORD
            })),
            false,
        )
        .await;
    assert_eq!(login.status(), StatusCode::OK);
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
    (account_id, cookie, csrf)
}

async fn identity_permission(app: &App, account_id: i64, allow: bool) {
    let response = app.call("PUT", &format!("/api/v1/admin/accounts/{account_id}/permissions"), Some(json!({
        "current_password": FIRST_PASSWORD, "can_view_identity": allow,
        "can_manage_permissions": false, "can_create_accounts": false, "can_manage_mail_settings": false
    })), true).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn basic_account_can_read_but_cannot_change_any_rental_data() {
    let app = App::new().await;
    let (account_id, cookie, csrf) = basic_account(&app).await;
    let asset: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ('Sál',0,0,'office@example.test') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    let block: i64 = sqlx::query_scalar("INSERT INTO rental_blocks(asset_id,start_date,end_date) VALUES ($1,DATE '2090-01-02',DATE '2090-01-02') RETURNING id")
        .bind(asset).fetch_one(&app.state.pool).await.unwrap();
    let approved: i64 = sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter) VALUES ($1,CURRENT_DATE-30,CURRENT_DATE-30,'Test','test@example.test','123456','Test','approved',0,'now','now',FALSE) RETURNING id")
        .bind(asset).fetch_one(&app.state.pool).await.unwrap();
    let invoiced: i64 = sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,status,rental_price,created_at,identity_consent_at,require_meter,identity,identity_type) VALUES ($1,CURRENT_DATE-31,CURRENT_DATE-31,'Test','test@example.test','123456','Test','invoiced',0,'now','now',FALSE,$2,'image/jpeg') RETURNING id")
        .bind(asset).bind(vec![1_u8,2,3]).fetch_one(&app.state.pool).await.unwrap();

    for path in [
        "/api/v1/admin/rental-assets".to_owned(),
        "/api/v1/admin/rentals".to_owned(),
        format!("/api/v1/admin/rental-assets/{asset}/blocks"),
        format!("/api/v1/admin/rentals/{approved}"),
    ] {
        assert_eq!(
            request_as(&app, &cookie, &csrf, "GET", &path, None)
                .await
                .status(),
            StatusCode::OK
        );
    }
    let input = json!({"name":"Upravený sál", "description":"", "rates":{"nonprofit":{"hourly_price":0,"weekend_day_price":0},"standard":{"hourly_price":0,"weekend_day_price":0}},"notification_email":"office@example.test","require_meter":false,"active":true});
    let changes = vec![
        (
            "POST",
            "/api/v1/admin/rental-assets".to_owned(),
            Some(input.clone()),
        ),
        (
            "PUT",
            format!("/api/v1/admin/rental-assets/{asset}"),
            Some(input),
        ),
        (
            "POST",
            format!("/api/v1/admin/rental-assets/{asset}/blocks"),
            Some(json!({"start_date":"2090-01-01","end_date":"2090-01-01"})),
        ),
        (
            "DELETE",
            format!("/api/v1/admin/rental-assets/{asset}/blocks/{block}"),
            None,
        ),
        (
            "POST",
            format!("/api/v1/admin/rentals/{approved}/invoice"),
            Some(
                json!({"energy_kwh":"0","energy_cost":0,"extra_cost":0,"account_iban":"","due_days":14}),
            ),
        ),
        (
            "POST",
            format!("/api/v1/admin/rentals/{invoiced}/paid"),
            None,
        ),
    ];
    for (method, path, body) in &changes {
        assert_eq!(
            request_as(&app, &cookie, &csrf, method, path, body.clone())
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
    let name: String = sqlx::query_scalar("SELECT name FROM rental_assets WHERE id=$1")
        .bind(asset)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(name, "Sál");
    let status: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
        .bind(invoiced)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "invoiced");
    let identity_exists: bool =
        sqlx::query_scalar("SELECT identity IS NOT NULL FROM rentals WHERE id=$1")
            .bind(invoiced)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(identity_exists);

    identity_permission(&app, account_id, true).await;
    for (method, path, body) in &changes {
        assert!(
            request_as(&app, &cookie, &csrf, method, path, body.clone())
                .await
                .status()
                .is_success(),
            "{method} {path}"
        );
    }
    identity_permission(&app, account_id, false).await;
    for (method, path, body) in &changes {
        assert_eq!(
            request_as(&app, &cookie, &csrf, method, path, body.clone())
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn viewing_identity_is_audited_with_actor_time_and_rental_id() {
    let app = App::new().await;
    let (account_id, cookie, csrf) = basic_account(&app).await;
    let asset: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ('Sál',0,0,'office@example.test') RETURNING id").fetch_one(&app.state.pool).await.unwrap();
    let rental = pending(&app, asset, 60).await;
    sqlx::query("UPDATE rentals SET identity=$1,identity_type='image/jpeg' WHERE id=$2")
        .bind(vec![1_u8, 2, 3])
        .bind(rental)
        .execute(&app.state.pool)
        .await
        .unwrap();
    identity_permission(&app, account_id, true).await;
    let path = format!("/api/v1/admin/rentals/{rental}/identity");
    assert_eq!(
        request_as(
            &app,
            &cookie,
            &csrf,
            "POST",
            &path,
            Some(json!({"password":"wrong"}))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request_as(
            &app,
            &cookie,
            &csrf,
            "POST",
            &path,
            Some(json!({"password":SECOND_PASSWORD}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let event: (i64, String, i64, String) = sqlx::query_as("SELECT actor_id,entity_type,entity_id,occurred_at FROM audit_log WHERE operation='viewed_identity' AND entity_id=$1").bind(rental).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(
        (event.0, event.1.as_str(), event.2),
        (account_id, "rental", rental)
    );
    assert!(!event.3.is_empty());
    identity_permission(&app, account_id, false).await;
    assert_eq!(
        request_as(
            &app,
            &cookie,
            &csrf,
            "POST",
            &path,
            Some(json!({"password":SECOND_PASSWORD}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE operation='viewed_identity' AND entity_id=$1",
    )
    .bind(rental)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn decision_requires_identity_permission_even_after_session_is_created() {
    let app = App::new().await;
    let account = app.call("POST", "/api/v1/admin/accounts", Some(json!({
        "email": "dalsi@vysker.test", "password": SECOND_PASSWORD, "current_password": FIRST_PASSWORD
    })), true).await;
    assert_eq!(account.status(), StatusCode::CREATED);
    let account_id = body_json(account).await["id"].as_i64().unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({
                "email": "dalsi@vysker.test", "password": SECOND_PASSWORD
            })),
            false,
        )
        .await;
    assert_eq!(login.status(), StatusCode::OK);
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
    let asset: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ('Sál',0,0,'office@example.test') RETURNING id")
        .fetch_one(&app.state.pool).await.unwrap();
    let first = pending(&app, asset, 60).await;
    let second = pending(&app, asset, 61).await;
    let third = pending(&app, asset, 62).await;
    let fourth = pending(&app, asset, 63).await;

    let detail = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/admin/rentals/{first}"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);

    for (id, action) in [(first, "approve"), (second, "reject")] {
        assert_eq!(
            decision(&app, &cookie, &csrf, id, action).await.status(),
            StatusCode::FORBIDDEN
        );
        let status: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
            .bind(id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(status, "pending");
    }
    let permission_path = format!("/api/v1/admin/accounts/{account_id}/permissions");
    let permissions = |allowed| {
        json!({"current_password": FIRST_PASSWORD, "can_view_identity": allowed,
        "can_manage_permissions": false, "can_create_accounts": false, "can_manage_mail_settings": false})
    };
    assert_eq!(
        app.call("PUT", &permission_path, Some(permissions(true)), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        decision(&app, &cookie, &csrf, first, "approve")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        decision(&app, &cookie, &csrf, second, "reject")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call("PUT", &permission_path, Some(permissions(false)), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        decision(&app, &cookie, &csrf, third, "approve")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        decision(&app, &cookie, &csrf, fourth, "reject")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let states: Vec<String> =
        sqlx::query_scalar("SELECT status FROM rentals WHERE id IN ($1,$2,$3,$4) ORDER BY id")
            .bind(first)
            .bind(second)
            .bind(third)
            .bind(fourth)
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(
        states,
        vec![
            "approved".to_owned(),
            "rejected".to_owned(),
            "pending".to_owned(),
            "pending".to_owned()
        ]
    );
}

#[tokio::test]
async fn revoked_session_cannot_finish_a_rental_write_waiting_for_the_lock() {
    let app = App::new().await;
    let mut lock = obecni_web::db::begin_write(&app.state.pool).await.unwrap();
    let lock_holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let router = app.router.clone();
    let cookie = app.cookie.clone();
    let csrf = app.csrf.clone();
    let waiting = tokio::spawn(async move {
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/admin/rental-assets")
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name":"Čekající sál", "description":"", "rates":{"nonprofit":{"hourly_price":0,"weekend_day_price":0},"standard":{"hourly_price":0,"weekend_day_price":0}}, "notification_email":"office@example.test",
                            "require_meter":false, "active":true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(lock_holder)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the authenticated request must wait for the transaction lock");
    sqlx::query("DELETE FROM sessions WHERE administrator_id=$1")
        .bind(app.admin_id)
        .execute(&mut *lock)
        .await
        .unwrap();
    lock.commit().await.unwrap();
    assert_eq!(waiting.await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rental_assets WHERE name=$1")
        .bind("Čekající sál")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
