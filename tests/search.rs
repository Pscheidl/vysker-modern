#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::{
    backend::{self, search},
    search::SearchQuery,
};

#[tokio::test]
async fn pagination_reaches_old_records_and_details_are_independent() {
    let app = App::new().await;
    let today = backend::today(time::OffsetDateTime::now_utc());
    sqlx::query("WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<1025) INSERT INTO notices(title,published_on,status) SELECT 'Vyhláška '||n,$1,'published' FROM seq").bind(&today).execute(&app.state.pool).await.unwrap();
    let q = SearchQuery {
        kind: "notice".into(),
        state: "current".into(),
        ..Default::default()
    };
    let first = search::query(&app.state.pool, &q).await.unwrap();
    assert_eq!(first.total, 1025);
    assert_eq!(first.items.len(), 20);
    let last = search::query(
        &app.state.pool,
        &SearchQuery {
            page: 52,
            ..q.clone()
        },
    )
    .await
    .unwrap();
    assert_eq!(last.items.len(), 5);
    assert_eq!(last.items.last().unwrap().id, 1);
    assert!(
        obecni_web::content::notice_from_pool(&app.state.pool, 1)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        search::query(
            &app.state.pool,
            &SearchQuery {
                page: usize::MAX,
                ..q
            }
        )
        .await
        .unwrap()
        .page,
        52
    );
}

#[tokio::test]
async fn search_is_folded_literal_multiword_and_never_matches_private_archive_fields() {
    let app = App::new().await;
    let today = backend::today(time::OffsetDateTime::now_utc());
    sqlx::query("INSERT INTO notices(title,description,reference_number,published_on,status,review_json) VALUES ('ŽÁDOST o pronájem','Příliš žluťoučký kůň','OU-100%',$1,'published','{}')").bind(&today).execute(&app.state.pool).await.unwrap();
    let q = SearchQuery {
        q: "zadost prilis kun".into(),
        state: "all".into(),
        ..Default::default()
    };
    assert_eq!(search::query(&app.state.pool, &q).await.unwrap().total, 1);
    assert_eq!(
        search::query(
            &app.state.pool,
            &SearchQuery {
                q: "%".into(),
                ..q.clone()
            }
        )
        .await
        .unwrap()
        .total,
        1
    );
    assert_eq!(
        search::query(
            &app.state.pool,
            &SearchQuery {
                q: "_".into(),
                ..q.clone()
            }
        )
        .await
        .unwrap()
        .total,
        0
    );
    // Expiry must hide original fields even before the maintenance worker runs.
    sqlx::query("UPDATE notices SET published_on='2020-01-01',withdraw_on=$1")
        .bind(&today)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(search::query(&app.state.pool, &q).await.unwrap().total, 0);
    let all = search::query(
        &app.state.pool,
        &SearchQuery {
            state: "all".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(all.total, 1);
    assert_eq!(all.items[0].title, "Záznam úřední desky #1");
    assert!(all.items[0].description.is_empty());
    assert!(all.items[0].archived);
    sqlx::query("UPDATE notices SET review_json='{\"archive_title\":\"Záměr obce\"}'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        search::query(
            &app.state.pool,
            &SearchQuery {
                q: "zamer".into(),
                ..q
            }
        )
        .await
        .unwrap()
        .total,
        1
    );
}

#[tokio::test]
async fn search_includes_only_published_documents_and_pages_and_validates_filters() {
    let app = App::new().await;
    sqlx::raw_sql("INSERT INTO documents(title,description,status,created_at) VALUES ('Poplatky','Místní poplatky','published','2026-09-29'),('Poplatky tajné','','draft','2026-09-29'); INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('poplatky','Poplatky','Úhrada poplatků',TRUE,'2026-09-29'),('tajne','Poplatky interní','Tajné',FALSE,'2026-09-29');").execute(&app.state.pool).await.unwrap();
    let response = app
        .call("GET", "/api/v1/search?q=poplatky&state=all", None, false)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["total"], 2);
    assert_eq!(
        app.call("GET", "/api/v1/search?sort=bad", None, false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let q = SearchQuery {
        q: "a ".repeat(11),
        ..Default::default()
    };
    assert!(search::query(&app.state.pool, &q).await.is_err());
}
