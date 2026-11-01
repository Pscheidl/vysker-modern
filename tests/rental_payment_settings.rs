#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{App, body_json};
use serde_json::{Value, json};
use tower::ServiceExt;

const PATH: &str = "/api/v1/admin/rental-payment-settings";
const FIRST: &str = "Testovaci-dlouhe-heslo-123456!";
const SECOND: &str = "Dalsi-velmi-dlouhe-heslo-123456!";

async fn request_as(
    app: &App,
    cookie: &str,
    csrf: &str,
    method: &str,
    value: Value,
) -> axum::response::Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(PATH)
                .header("cookie", cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn payment_defaults_validate_accounts_and_require_current_rental_permission() {
    let app = App::new().await;
    assert_eq!(
        app.call("GET", PATH, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let defaults = body_json(app.call("GET", PATH, None, true).await).await;
    assert_eq!(
        defaults,
        json!({"account_number":"", "bank_code":"", "iban":"", "due_days":7, "effective_iban":""})
    );
    let valid =
        json!({"account_number":"000019-2000145399", "bank_code":"0800", "iban":"", "due_days":7});
    let saved = app.call("PUT", PATH, Some(valid.clone()), true).await;
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = body_json(saved).await;
    assert_eq!(saved["account_number"], "19-2000145399");
    assert_eq!(saved["bank_code"], "0800");
    assert_eq!(saved["iban"], "");
    assert_eq!(saved["effective_iban"], "CZ6508000000192000145399");

    for invalid in [
        json!({"account_number":"19-2000145398","bank_code":"0800","iban":"","due_days":7}),
        json!({"account_number":"18-2000145399","bank_code":"0800","iban":"","due_days":7}),
        json!({"account_number":"0","bank_code":"0800","iban":"","due_days":7}),
        json!({"account_number":"19-2000145399","bank_code":"800","iban":"","due_days":7}),
        json!({"account_number":"19-2000145399","bank_code":"0100","iban":"CZ6508000000192000145399","due_days":7}),
        json!({"account_number":"19-2000145399","bank_code":"0800","iban":"","due_days":0}),
    ] {
        assert_eq!(
            app.call("PUT", PATH, Some(invalid), true).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        body_json(app.call("GET", PATH, None, true).await).await,
        saved
    );
    let response = app.call("PUT", PATH, Some(json!({"account_number":"19-2000145399","bank_code":"0800","iban":"cz65 0800 0000 1920 0014 5399","due_days":20})), true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await["iban"],
        "CZ6508000000192000145399"
    );

    let created = app
        .call(
            "POST",
            "/api/v1/admin/accounts",
            Some(json!({"email":"basic@vysker.test","password":SECOND,"current_password":FIRST})),
            true,
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_i64().unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"basic@vysker.test","password":SECOND})),
            false,
        )
        .await;
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
    assert_eq!(
        request_as(&app, &cookie, &csrf, "GET", Value::Null)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request_as(&app, &cookie, &csrf, "PUT", valid.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let permission = |allowed| json!({"current_password":FIRST,"can_view_identity":allowed,"can_manage_permissions":false,"can_create_accounts":false,"can_manage_mail_settings":false});
    let path = format!("/api/v1/admin/accounts/{id}/permissions");
    assert_eq!(
        app.call("PUT", &path, Some(permission(true)), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request_as(&app, &cookie, &csrf, "PUT", valid.clone())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call("PUT", &path, Some(permission(false)), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request_as(&app, &cookie, &csrf, "PUT", valid)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}
