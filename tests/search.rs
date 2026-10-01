#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::{
    backend::{self, search},
    search::SearchQuery,
};

#[tokio::test]
async fn imported_notices_without_dates_are_visible_in_the_board_archive() {
    let app = App::new().await;
    sqlx::raw_sql(
        "INSERT INTO notices(id,title,published_on,status) VALUES
        (1,'Dotace bez data',NULL,'archived'),
        (2,'Datované vyvěšení','2020-01-01','archived'),
        (3,'Soukromý koncept',NULL,'draft');
        UPDATE notices SET retain_attachments=TRUE,
        review_json=jsonb_build_object('archive_title',title,'archive_basis','Místní náhled migrace','archive_until','9999-12-31')::text
        WHERE status='archived';
        INSERT INTO attachments(notice_id,name,content_type,size_bytes,data) VALUES
        (1,'dotace.pdf','application/pdf',4,'data')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let query = SearchQuery {
        kind: "notice".into(),
        state: "archive".into(),
        ..Default::default()
    };
    let current = search::query(
        &app.state.pool,
        &SearchQuery {
            state: "current".into(),
            ..query.clone()
        },
    )
    .await
    .unwrap();
    assert_eq!(current.total, 0);
    for sort in ["newest", "oldest"] {
        let result = search::query(
            &app.state.pool,
            &SearchQuery {
                sort: sort.into(),
                ..query.clone()
            },
        )
        .await
        .unwrap();
        assert_eq!(result.total, 2);
        assert_eq!(result.items[0].id, 2);
        assert_eq!(result.items[1].id, 1);
        assert_eq!(result.items[1].posted, "");
    }
    let docs = search::query(
        &app.state.pool,
        &SearchQuery {
            kind: "document".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(docs.total, 0);
    let admin = body_json(
        app.call(
            "GET",
            "/api/v1/admin/notices?status=archived&limit=21&offset=0",
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(admin[0]["id"], 2);
    assert_eq!(admin[1]["id"], 1);
    let detail = body_json(app.call("GET", "/api/v1/notices/1", None, false).await).await;
    assert!(detail["published_on"].is_null());
    assert!(detail["withdraw_on"].is_null());
    assert_eq!(detail["status"], "archived");
    assert_eq!(detail["attachments"][0]["available"], true);
    assert_eq!(
        app.call("GET", "/api/v1/attachments/1", None, false)
            .await
            .status(),
        StatusCode::OK
    );
    let presented = obecni_web::content::notice_from_pool(&app.state.pool, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(presented.posted, "Datum vyvěšení není uvedeno");
    assert_eq!(presented.ends, "Datum sejmutí není uvedeno");
    assert!(presented.ends_iso.is_none());
    assert!(presented.archived);
    // A known future posting date must not hide an already archived import.
    sqlx::query("UPDATE notices SET published_on='9998-01-01' WHERE id=2")
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        search::query(&app.state.pool, &query).await.unwrap().total,
        2
    );
    assert_eq!(
        app.call("GET", "/api/v1/notices/2", None, false)
            .await
            .status(),
        StatusCode::OK
    );
    assert!(
        obecni_web::content::notice_from_pool(&app.state.pool, 2)
            .await
            .unwrap()
            .unwrap()
            .archived
    );
    assert_eq!(app.publish(3).await.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn documents_sort_by_original_dates_with_unknown_dates_last() {
    let app = App::new().await;
    sqlx::raw_sql(
        "INSERT INTO documents(title,status,created_at,source_published_on,published_at) VALUES
        ('Old source','published','2026-10-01','2019-03-01',NULL),
        ('Recent source','published','2026-10-01','2026-09-24',NULL),
        ('Native publication','published','2026-09-25',NULL,'2026-09-25T12:00:00Z'),
        ('Unknown source date','published','2026-10-01',NULL,NULL)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let q = SearchQuery {
        kind: "document".into(),
        ..Default::default()
    };
    let newest = search::query(&app.state.pool, &q).await.unwrap();
    assert_eq!(
        newest
            .items
            .iter()
            .map(|h| h.title.as_str())
            .collect::<Vec<_>>(),
        [
            "Native publication",
            "Recent source",
            "Old source",
            "Unknown source date"
        ]
    );
    let oldest = search::query(
        &app.state.pool,
        &SearchQuery {
            sort: "oldest".into(),
            ..q
        },
    )
    .await
    .unwrap();
    assert_eq!(
        oldest
            .items
            .iter()
            .map(|h| h.title.as_str())
            .collect::<Vec<_>>(),
        [
            "Old source",
            "Recent source",
            "Native publication",
            "Unknown source date"
        ]
    );
    let api = body_json(app.call("GET", "/api/v1/documents", None, false).await).await;
    assert_eq!(api[0]["title"], "Native publication");
    assert_eq!(api[1]["source_published_on"], "2026-09-24");
    assert!(api[1]["published_at"].is_null());
    let admin = body_json(
        app.call(
            "GET",
            "/api/v1/admin/documents?status=published&limit=21&offset=0",
            None,
            true,
        )
        .await,
    )
    .await;
    assert_eq!(admin[0]["title"], "Native publication");
    assert_eq!(admin[3]["title"], "Unknown source date");
}

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
