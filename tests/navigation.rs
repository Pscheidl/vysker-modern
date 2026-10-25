#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::catalog::navigation_from_pool;
use serde_json::{Value, json};

const NAVIGATION: &str = "/api/v1/admin/navigation";

fn item(label: &str, parent_id: Option<i64>, sort_order: i64) -> Value {
    json!({
        "label": label,
        "path": "/kontakt",
        "sort_order": sort_order,
        "visible": true,
        "parent_id": parent_id
    })
}

async fn create(app: &App, input: Value) -> i64 {
    let response = app.call("POST", NAVIGATION, Some(input), true).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await["id"].as_i64().unwrap()
}

async fn update(app: &App, id: i64, mut input: Value, version: i64) -> axum::response::Response {
    input["expected_version"] = json!(version);
    app.call("PUT", &format!("{NAVIGATION}/{id}"), Some(input), true)
        .await
}

#[tokio::test]
async fn public_navigation_groups_and_orders_children_and_hides_whole_branches() {
    let app = App::new().await;
    let default_count = navigation_from_pool(&app.state.pool).await.unwrap().len();
    let root = create(&app, item("Obec", None, 0)).await;
    let late = create(&app, item("Později", Some(root), 20)).await;
    let early = create(&app, item("Nejdříve", Some(root), 0)).await;
    let tied = create(&app, item("Stejné pořadí", Some(root), 0)).await;
    let mut hidden = item("Skryté", Some(root), 0);
    hidden["visible"] = json!(false);
    create(&app, hidden).await;

    let navigation = navigation_from_pool(&app.state.pool).await.unwrap();
    assert_eq!(navigation[0].id, root);
    assert_eq!(
        navigation[0]
            .children
            .iter()
            .map(|child| child.id)
            .collect::<Vec<_>>(),
        [early, tied, late]
    );
    assert!(navigation.iter().all(|link| link.id != early));
    assert_eq!(navigation.len(), default_count + 1);

    let listed = body_json(app.call("GET", NAVIGATION, None, true).await).await;
    let child = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == early)
        .unwrap();
    assert_eq!(child["parent_id"], root);

    let mut root_input = item("Obec", None, 0);
    root_input["visible"] = json!(false);
    assert_eq!(
        update(&app, root, root_input.clone(), 1).await.status(),
        StatusCode::NO_CONTENT
    );
    let navigation = navigation_from_pool(&app.state.pool).await.unwrap();
    assert_eq!(navigation.len(), default_count);
    assert!(navigation.iter().all(|link| link.id != root));

    root_input["visible"] = json!(true);
    assert_eq!(
        update(&app, root, root_input, 2).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        navigation_from_pool(&app.state.pool).await.unwrap()[0]
            .children
            .len(),
        3
    );
}

#[tokio::test]
async fn unpublished_pages_hide_children_and_parents_until_republished() {
    let app = App::new().await;
    let mut page = json!({
        "slug": "spolky",
        "title": "Spolky",
        "content": "Obsah",
        "published": true
    });
    let response = app
        .call("POST", "/api/v1/admin/pages", Some(page.clone()), true)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let page_id = body_json(response).await["id"].as_i64().unwrap();
    let mut root_input = item("Spolky", None, 0);
    root_input["path"] = json!("/stranky/spolky");
    let root = create(&app, root_input).await;
    create(&app, item("Kontakt na spolky", Some(root), 0)).await;
    let other_root = create(&app, item("Další informace", None, 1)).await;
    let mut page_child = item("Spolky", Some(other_root), 0);
    page_child["path"] = json!("/stranky/spolky");
    create(&app, page_child).await;

    page["published"] = json!(false);
    page["expected_version"] = json!(1);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/pages/{page_id}"),
            Some(page.clone()),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let navigation = navigation_from_pool(&app.state.pool).await.unwrap();
    assert!(navigation.iter().all(|link| link.id != root));
    assert!(
        navigation
            .iter()
            .find(|link| link.id == other_root)
            .unwrap()
            .children
            .is_empty()
    );

    page["published"] = json!(true);
    page["expected_version"] = json!(2);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/pages/{page_id}"),
            Some(page),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let navigation = navigation_from_pool(&app.state.pool).await.unwrap();
    assert_eq!(navigation[0].id, root);
    assert_eq!(navigation[0].children.len(), 1);
    assert_eq!(navigation[1].children.len(), 1);
}

#[tokio::test]
async fn hierarchy_validation_blocks_missing_parents_cycles_depth_and_parent_deletion() {
    let app = App::new().await;
    let root = create(&app, item("Obec", None, 0)).await;
    let other = create(&app, item("Úřad", None, 1)).await;
    let child = create(&app, item("Kontakt", Some(root), 0)).await;

    for invalid_parent in [i64::MAX, child] {
        assert_eq!(
            app.call(
                "POST",
                NAVIGATION,
                Some(item("Nepřípustné", Some(invalid_parent), 0)),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for invalid_parent in [root, child, other] {
        assert_eq!(
            update(&app, root, item("Obec", Some(invalid_parent), 0), 1)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let response = app
        .call(
            "DELETE",
            &format!("{NAVIGATION}/{root}"),
            Some(json!({"expected_version": 1})),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        body_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("podmenu")
    );

    assert_eq!(
        update(&app, child, item("Kontakt", Some(other), 0), 1)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        update(&app, child, item("Stará změna", None, 0), 1)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{NAVIGATION}/{child}"),
            Some(json!({"expected_version": 1})),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{NAVIGATION}/{root}"),
            Some(json!({"expected_version": 1})),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        update(&app, child, item("Samostatný kontakt", None, 0), 2)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn concurrent_moves_cannot_create_cycles_or_overwrite_a_new_version() {
    let app = App::new().await;
    let first = create(&app, item("První", None, 0)).await;
    let second = create(&app, item("Druhá", None, 1)).await;
    let (a, b) = tokio::join!(
        update(&app, first, item("První", Some(second), 0), 1),
        update(&app, second, item("Druhá", Some(first), 1), 1)
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [204, 400]);
    let (root, child): (i64, i64) = sqlx::query_as(
        "SELECT parent_id,id FROM navigation_items WHERE id IN ($1,$2) AND parent_id IS NOT NULL",
    )
    .bind(first)
    .bind(second)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    let (a, b) = tokio::join!(
        update(&app, child, item("Jedna změna", Some(root), 0), 2),
        update(&app, child, item("Druhá změna", Some(root), 0), 2)
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [204, 409]);
}

#[tokio::test]
async fn navigation_keeps_existing_root_capacity_and_limits_total_size() {
    let app = App::new().await;
    sqlx::query(
        "INSERT INTO navigation_items(label,path,sort_order)
         SELECT 'Hlavní položka '||i,'/kontakt',i
         FROM generate_series(1,20-(SELECT count(*) FROM navigation_items WHERE parent_id IS NULL)) AS i",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        app.call("POST", NAVIGATION, Some(item("Navíc", None, 0)), true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let root: i64 = sqlx::query_scalar("SELECT min(id) FROM navigation_items")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let child = create(&app, item("Podmenu", Some(root), 0)).await;
    assert_eq!(
        update(&app, child, item("Nová hlavní položka", None, 0), 1)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    sqlx::query(
        "INSERT INTO navigation_items(label,path,sort_order,parent_id)
         SELECT 'Podřízená položka '||i,'/kontakt',i,$1 FROM generate_series(1,39) AS i",
    )
    .bind(root)
    .execute(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        app.call(
            "POST",
            NAVIGATION,
            Some(item("Nad limit", Some(root), 0)),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        update(&app, child, item("Přejmenované podmenu", Some(root), 0), 1)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let navigation = navigation_from_pool(&app.state.pool).await.unwrap();
    assert_eq!(navigation.len(), 20);
    assert_eq!(
        navigation
            .iter()
            .map(|link| link.children.len())
            .sum::<usize>(),
        40
    );
}

#[tokio::test]
async fn issue_reports_have_a_default_menu_link_and_can_be_managed() {
    let app = App::new().await;
    let navigation = navigation_from_pool(&app.state.pool).await.unwrap();
    let report_link = navigation
        .iter()
        .find(|link| link.path == "/hlaseni-zavad")
        .expect("new installations expose public issue reports in the menu");
    assert_eq!(report_link.label, "Podněty a závady");
    assert!(
        obecni_web::catalog::default_navigation()
            .iter()
            .any(|link| link.path == "/hlaseni-zavad" && link.label == "Podněty a závady")
    );

    let admin_navigation = body_json(app.call("GET", NAVIGATION, None, true).await).await;
    let version = admin_navigation
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == report_link.id)
        .unwrap()["version"]
        .as_i64()
        .unwrap();
    let mut input = item("Závady v obci", None, 50);
    input["path"] = json!("/hlaseni-zavad");
    assert_eq!(
        update(&app, report_link.id, input, version).await.status(),
        StatusCode::NO_CONTENT
    );
    let navigation = navigation_from_pool(&app.state.pool).await.unwrap();
    assert!(
        navigation
            .iter()
            .any(|link| { link.path == "/hlaseni-zavad" && link.label == "Závady v obci" })
    );
}
