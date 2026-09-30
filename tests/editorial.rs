#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use serde_json::json;

#[tokio::test]
async fn pages_preserve_revisions_and_reject_concurrent_overwrite() {
    let app = App::new().await;
    let input = json!({"slug":"spolky","title":"Spolky","content":"## Nadpis\n\n**Text**","published":true});
    assert_eq!(
        app.call("POST", "/api/v1/admin/pages", Some(input.clone()), false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = app
        .call("POST", "/api/v1/admin/pages", Some(input.clone()), true)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = body_json(response).await["id"].as_i64().unwrap();
    let path = format!("/api/v1/admin/pages/{id}");
    let mut update = input.clone();
    update["expected_version"] = json!(1);
    update["content"] = json!("Změna");
    let (a, b) = tokio::join!(
        app.call("PUT", &path, Some(update.clone()), true),
        app.call("PUT", &path, Some(update), true)
    );
    let mut statuses = vec![a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [204, 409]);
    let page = body_json(app.call("GET", &path, None, true).await).await;
    assert_eq!(page["version"], 2);
    let history = format!("{path}/revisions");
    assert_eq!(
        app.call("GET", &history, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let revisions = body_json(app.call("GET", &history, None, true).await).await;
    assert_eq!(revisions.as_array().unwrap().len(), 2);
    assert_eq!(revisions[1]["content"], input["content"]);
    let mut restored = input.clone();
    restored["expected_version"] = json!(2);
    assert_eq!(
        app.call("PUT", &path, Some(restored), true).await.status(),
        StatusCode::NO_CONTENT
    );
    let revisions = body_json(app.call("GET", &history, None, true).await).await;
    assert_eq!(revisions[0]["version"], 3);
    assert!(
        sqlx::query("UPDATE page_revisions SET content='tampered'")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM page_revisions")
            .execute(&app.state.pool)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn navigation_accepts_only_existing_local_pages_and_tracks_renames() {
    let app = App::new().await;
    let path = "/api/v1/admin/navigation";
    assert_eq!(
        app.call("GET", path, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for link in [
        "javascript:alert(1)",
        "https://example.test",
        "//evil.test",
        "/admin",
        "/stranky/missing",
        "/stranky/%2e%2e/admin",
    ] {
        assert_eq!(
            app.call(
                "POST",
                path,
                Some(json!({"label":"Test","path":link,"sort_order":1,"visible":true})),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "{link}"
        );
    }
    let mut page = json!({"slug":"spolky","title":"Spolky","content":"Obsah","published":false});
    let id = body_json(
        app.call("POST", "/api/v1/admin/pages", Some(page.clone()), true)
            .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    let input = json!({"label":"Spolky","path":"/stranky/spolky","sort_order":1,"visible":true});
    assert_eq!(
        app.call("POST", path, Some(input.clone()), true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    page["published"] = json!(true);
    page["expected_version"] = json!(1);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/pages/{id}"),
            Some(page.clone()),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let response = app.call("POST", path, Some(input.clone()), true).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let menu_id = body_json(response).await["id"].as_i64().unwrap();
    page["slug"] = json!("spolky-nove");
    page["expected_version"] = json!(2);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/pages/{id}"),
            Some(page),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let (link, version): (String, i64) =
        sqlx::query_as("SELECT path,version FROM navigation_items WHERE id=$1")
            .bind(menu_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(link, "/stranky/spolky-nove");
    assert_eq!(version, 2);
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{path}/{menu_id}"),
            Some(json!({"expected_version":1})),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{path}/{menu_id}"),
            Some(json!({"expected_version":2})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn categories_are_audited_ordered_and_used_categories_cannot_be_deleted() {
    let app = App::new().await;
    let path = "/api/v1/admin/categories";
    assert_eq!(
        app.call("GET", path, None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let input = json!({"name":"Nová kategorie","sort_order":0});
    let response = app.call("POST", path, Some(input), true).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = body_json(response).await["id"].as_i64().unwrap();
    let detail = format!("{path}/{id}");
    let input = json!({"name":"Upravená","sort_order":1,"expected_version":1});
    assert_eq!(
        app.call("PUT", &detail, Some(input.clone()), true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("PUT", &detail, Some(input), true).await.status(),
        StatusCode::CONFLICT
    );
    let categories = body_json(app.call("GET", "/api/v1/categories", None, false).await).await;
    assert_eq!(categories[0]["name"], "Upravená");
    let _ = app.notice(json!({"category_id":id})).await;
    assert_eq!(
        app.call("DELETE", &detail, Some(json!({"expected_version":2})), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log WHERE entity_type='category'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap(),
        2
    );
}
