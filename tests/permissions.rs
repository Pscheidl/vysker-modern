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

async fn other_request(
    app: &App,
    cookie: &str,
    csrf: &str,
    method: &str,
    path: &str,
    value: Value,
) -> Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
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
async fn first_admin_is_protected_and_other_permissions_are_explicit_and_immediate() {
    let app = App::new().await;
    let session = body_json(app.call("GET", "/api/v1/admin/session", None, true).await).await;
    assert_eq!(session["is_primary"], true);
    assert_eq!(session["can_manage_permissions"], true);
    assert_eq!(session["can_view_identity"], true);
    assert_eq!(session["can_create_accounts"], true);
    assert_eq!(session["can_manage_mail_settings"], true);
    let created = app.call("POST", "/api/v1/admin/accounts", Some(json!({
        "email":"druhy@vysker.test", "password":SECOND_PASSWORD, "current_password":FIRST_PASSWORD
    })), true).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let second = body_json(created).await["id"].as_i64().unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"druhy@vysker.test", "password":SECOND_PASSWORD})),
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
    let session = body_json(login).await;
    let csrf = session["csrf_token"].as_str().unwrap();
    assert_eq!(session["is_primary"], false);
    assert_eq!(session["can_manage_permissions"], false);
    assert_eq!(session["can_view_identity"], false);
    assert_eq!(session["can_create_accounts"], false);
    assert_eq!(session["can_manage_mail_settings"], false);
    assert_eq!(other_request(&app,&cookie,csrf,"POST","/api/v1/admin/accounts",json!({
        "email":"zakladni@vysker.test", "password":SECOND_PASSWORD, "current_password":SECOND_PASSWORD
    })).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(other_request(&app,&cookie,csrf,"POST","/api/v1/admin/accounts",json!({
        "email":"evil@vysker.test", "password":SECOND_PASSWORD, "current_password":SECOND_PASSWORD, "can_manage_permissions":true
    })).await.status(), StatusCode::FORBIDDEN);
    let permission_path = format!("/api/v1/admin/accounts/{second}/permissions");
    let grant = json!({"current_password":FIRST_PASSWORD,"can_view_identity":true,"can_manage_permissions":false,"can_create_accounts":false,"can_manage_mail_settings":false});
    assert_eq!(other_request(&app,&cookie,csrf,"PUT",&permission_path,json!({"current_password":SECOND_PASSWORD,"can_view_identity":true,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":true})).await.status(),StatusCode::FORBIDDEN);
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &format!("/api/v1/admin/accounts/{}", app.admin_id),
            json!({"active":false,"current_password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "DELETE",
            &format!("/api/v1/admin/accounts/{}/sessions", app.admin_id),
            json!({"current_password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.call("PUT",&format!("/api/v1/admin/accounts/{}/permissions",app.admin_id),Some(json!({"current_password":FIRST_PASSWORD,"can_view_identity":false,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":true})),true).await.status(),StatusCode::CONFLICT);
    let asset: i64 = sqlx::query_scalar("INSERT INTO rental_assets(name,weekday_price,weekend_price,notification_email) VALUES ('Test',0,0,'admin@example.test') RETURNING id").fetch_one(&app.state.pool).await.unwrap();
    let id: i64 = sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,name,email,phone,address,rental_price,created_at,identity_consent_at,require_meter,identity,identity_type) VALUES ($1,CURRENT_DATE,CURRENT_DATE,'Test','test@example.test','123456','Test',0,'now','now',FALSE,$2,'image/jpeg') RETURNING id")
        .bind(asset).bind(vec![1_u8,2,3]).fetch_one(&app.state.pool).await.unwrap();
    let path = format!("/api/v1/admin/rentals/{id}/identity");
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "POST",
            &path,
            json!({"password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.call("PUT", &permission_path, Some(grant), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "POST",
            &path,
            json!({"password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(app.call("PUT",&permission_path,Some(json!({"current_password":FIRST_PASSWORD,"can_view_identity":false,"can_manage_permissions":false,"can_create_accounts":false,"can_manage_mail_settings":false})),true).await.status(),StatusCode::NO_CONTENT);
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "POST",
            &path,
            json!({"password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn password_policy_has_an_absolute_minimum() {
    assert!(
        obecni_web::backend::auth::password_hash("a".repeat(23), 1)
            .await
            .is_err()
    );
    assert!(
        obecni_web::backend::auth::password_hash("a".repeat(24), 1)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn delegated_accounts_can_change_only_rights_they_possess() {
    let app = App::new().await;
    let creator = app.call("POST", "/api/v1/admin/accounts", Some(json!({
        "email":"delegat@vysker.test", "password":SECOND_PASSWORD, "current_password":FIRST_PASSWORD,
        "can_create_accounts":true, "can_manage_permissions":true
    })), true).await;
    assert_eq!(creator.status(), StatusCode::CREATED);
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"delegat@vysker.test", "password":SECOND_PASSWORD})),
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
    let session = body_json(login).await;
    let csrf = session["csrf_token"].as_str().unwrap();
    assert_eq!(session["can_create_accounts"], true);
    assert_eq!(session["can_manage_permissions"], true);
    assert_eq!(session["can_view_identity"], false);
    assert_eq!(session["can_manage_mail_settings"], false);
    let base = json!({"email":"novy@vysker.test", "password":SECOND_PASSWORD, "current_password":SECOND_PASSWORD});
    let mut illicit = base.clone();
    illicit["can_view_identity"] = json!(true);
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "POST",
            "/api/v1/admin/accounts",
            illicit
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut illicit = base.clone();
    illicit["can_manage_mail_settings"] = json!(true);
    illicit["can_view_identity"] = json!(true);
    illicit["can_manage_permissions"] = json!(true);
    illicit["can_create_accounts"] = json!(true);
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "POST",
            "/api/v1/admin/accounts",
            illicit
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let created = other_request(&app, &cookie, csrf, "POST", "/api/v1/admin/accounts", base).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let target = body_json(created).await["id"].as_i64().unwrap();
    let path = format!("/api/v1/admin/accounts/{target}/permissions");
    let grant_view = json!({"current_password":FIRST_PASSWORD,"can_view_identity":true,"can_manage_permissions":false,"can_create_accounts":false,"can_manage_mail_settings":false});
    assert_eq!(
        app.call("PUT", &path, Some(grant_view), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let mut unchanged_view = json!({"current_password":SECOND_PASSWORD,"can_view_identity":true,"can_manage_permissions":false,"can_create_accounts":true,"can_manage_mail_settings":false});
    assert_eq!(
        other_request(&app, &cookie, csrf, "PUT", &path, unchanged_view.clone())
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    unchanged_view["can_view_identity"] = json!(false);
    assert_eq!(
        other_request(&app, &cookie, csrf, "PUT", &path, unchanged_view)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &format!("/api/v1/admin/accounts/{target}"),
            json!({"active":false,"current_password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "DELETE",
            &format!("/api/v1/admin/accounts/{target}/sessions"),
            json!({"current_password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn sender_permission_requires_all_other_permissions() {
    let app = App::new().await;
    let invalid_create = json!({"email":"mail@vysker.test","password":SECOND_PASSWORD,"current_password":FIRST_PASSWORD,"can_manage_mail_settings":true});
    assert_eq!(
        app.call("POST", "/api/v1/admin/accounts", Some(invalid_create), true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let created = app.call("POST","/api/v1/admin/accounts",Some(json!({"email":"mail@vysker.test","password":SECOND_PASSWORD,"current_password":FIRST_PASSWORD})),true).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_i64().unwrap();
    let path = format!("/api/v1/admin/accounts/{id}/permissions");
    let invalid = json!({"current_password":FIRST_PASSWORD,"can_view_identity":false,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":true});
    assert_eq!(
        app.call("PUT", &path, Some(invalid), true).await.status(),
        StatusCode::BAD_REQUEST
    );
    let all = json!({"current_password":FIRST_PASSWORD,"can_view_identity":true,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":true});
    assert_eq!(
        app.call("PUT", &path, Some(all), true).await.status(),
        StatusCode::NO_CONTENT
    );
    for missing in [
        "can_view_identity",
        "can_manage_permissions",
        "can_create_accounts",
    ] {
        let mut input = json!({"current_password":FIRST_PASSWORD,"can_view_identity":true,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":true});
        input[missing] = json!(false);
        assert_eq!(
            app.call("PUT", &path, Some(input), true).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert!(
        sqlx::query("UPDATE administrators SET can_view_identity=false WHERE id=$1")
            .bind(id)
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    let no_mail = json!({"current_password":FIRST_PASSWORD,"can_view_identity":true,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":false});
    assert_eq!(
        app.call("PUT", &path, Some(no_mail), true).await.status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn first_account_may_be_deactivated_only_while_another_full_admin_remains() {
    let app = App::new().await;
    let created = app.call("POST","/api/v1/admin/accounts",Some(json!({"email":"full@vysker.test","password":SECOND_PASSWORD,"current_password":FIRST_PASSWORD,"can_view_identity":true,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":true})),true).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let second = body_json(created).await["id"].as_i64().unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"full@vysker.test","password":SECOND_PASSWORD})),
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
    let session = body_json(login).await;
    let csrf = session["csrf_token"].as_str().unwrap();
    let primary = format!("/api/v1/admin/accounts/{}", app.admin_id);
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &primary,
            json!({"active":false,"current_password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", "/api/v1/admin/session", None, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let second_path = format!("/api/v1/admin/accounts/{second}");
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &second_path,
            json!({"active":false,"current_password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let remove = json!({"current_password":SECOND_PASSWORD,"can_view_identity":true,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":false});
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &format!("{second_path}/permissions"),
            remove.clone()
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &primary,
            json!({"active":true,"current_password":SECOND_PASSWORD})
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &format!("{second_path}/permissions"),
            remove
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let primary_rights: (bool,bool,bool,bool,bool) = sqlx::query_as("SELECT is_primary,can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings FROM administrators WHERE id=$1").bind(app.admin_id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(primary_rights, (true, true, true, true, true));
}

#[tokio::test]
async fn simultaneous_self_deactivations_cannot_remove_the_last_full_admin() {
    let app = App::new().await;
    let created = app.call("POST","/api/v1/admin/accounts",Some(json!({"email":"race@vysker.test","password":SECOND_PASSWORD,"current_password":FIRST_PASSWORD,"can_view_identity":true,"can_manage_permissions":true,"can_create_accounts":true,"can_manage_mail_settings":true})),true).await;
    let id = body_json(created).await["id"].as_i64().unwrap();
    let login = app
        .call(
            "POST",
            "/api/v1/admin/login",
            Some(json!({"email":"race@vysker.test","password":SECOND_PASSWORD})),
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
    let session = body_json(login).await;
    let csrf = session["csrf_token"].as_str().unwrap();
    let first = format!("/api/v1/admin/accounts/{}", app.admin_id);
    let second = format!("/api/v1/admin/accounts/{id}");
    let (a, b) = tokio::join!(
        app.call(
            "PUT",
            &first,
            Some(json!({"active":false,"current_password":FIRST_PASSWORD})),
            true
        ),
        other_request(
            &app,
            &cookie,
            csrf,
            "PUT",
            &second,
            json!({"active":false,"current_password":SECOND_PASSWORD})
        )
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [204, 409]);
    let full: i64 = sqlx::query_scalar("SELECT count(*) FROM administrators WHERE active AND can_view_identity AND can_manage_permissions AND can_create_accounts AND can_manage_mail_settings").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(full, 1);
}
