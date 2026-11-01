#![cfg(feature = "ssr")]
mod common;
use axum::http::StatusCode;
use common::{App, body_json};
use obecni_web::{backend::search, search::SearchQuery};

async fn document(app: &App, title: &str, board: Option<&str>, date: Option<&str>) -> i64 {
    sqlx::query_scalar("INSERT INTO documents(title,description,status,created_at,source_published_on,published_on,board_status,public_access) VALUES ($1,'Příliš žluťoučký kůň','published','2026-11-01T10:00:00Z',$2::text::date,$2::text::date,$3,TRUE) RETURNING id")
        .bind(title).bind(date).bind(board).fetch_one(&app.state.pool).await.unwrap()
}
async fn posting(app: &App, id: i64, removed: bool) {
    sqlx::query("UPDATE documents SET lock_state='locked' WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO document_postings(document_id,origin,certainty,started_on,removed_on,after_action,snapshot_sha256) SELECT id,'source','documented',published_on,CASE WHEN $2 THEN DATE '2026-10-01' ELSE NULL END,'retain',document_fingerprint(id) FROM documents WHERE id=$1")
        .bind(id).bind(removed).execute(&app.state.pool).await.unwrap();
}
fn scope(kind: &str, state: &str) -> SearchQuery {
    SearchQuery {
        kind: kind.into(),
        state: state.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn one_canonical_document_is_in_catalog_board_and_global_search_once() {
    let app = App::new().await;
    let ordinary = document(&app, "Běžný dokument", None, Some("2026-10-01")).await;
    let current = document(
        &app,
        "Aktuální písemnost",
        Some("published"),
        Some("2026-10-02"),
    )
    .await;
    posting(&app, current, false).await;
    let historical = document(
        &app,
        "Sejmutá písemnost",
        Some("archived"),
        Some("2020-01-01"),
    )
    .await;
    posting(&app, historical, true).await;
    // A status or a past planned date alone is not evidence of an actual posting.
    let uncertain = document(&app, "Nejasná historie", Some("archived"), None).await;
    sqlx::query("UPDATE documents SET withdraw_on='2020-01-15' WHERE id=$1")
        .bind(uncertain)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let catalog = search::query(&app.state.pool, &scope("document", "all"))
        .await
        .unwrap();
    assert_eq!(catalog.total, 4);
    assert!(catalog.items.iter().any(|h| h.id == ordinary));
    let board = search::query(&app.state.pool, &scope("notice", "current"))
        .await
        .unwrap();
    assert_eq!(
        board.items.iter().map(|h| h.id).collect::<Vec<_>>(),
        [current]
    );
    assert_eq!(board.items[0].path, format!("/dokumenty/{current}"));
    let compact = obecni_web::model::notices::published(&app.state.pool, 3)
        .await
        .unwrap();
    assert_eq!(compact.len(), 1);
    assert_eq!(compact[0].id, current);
    assert!(compact[0].status.is_published());
    let counts = obecni_web::model::notices::counts(&app.state.pool)
        .await
        .unwrap();
    assert_eq!((counts.published, counts.archived), (1, 1));
    let archive = search::query(&app.state.pool, &scope("notice", "archive"))
        .await
        .unwrap();
    assert_eq!(
        archive.items.iter().map(|h| h.id).collect::<Vec<_>>(),
        [historical]
    );
    let global = search::query(&app.state.pool, &SearchQuery::default())
        .await
        .unwrap();
    assert_eq!(global.total, 4);
    assert_eq!(global.items.iter().filter(|h| h.id == current).count(), 1);
    // Copies representing separate corrective documents remain separate results.
    let correction = document(&app, "Aktuální písemnost", None, Some("2026-10-02")).await;
    sqlx::query(
        "INSERT INTO document_relations(original_id,related_id,kind) VALUES ($1,$2,'correction')",
    )
    .bind(current)
    .bind(correction)
    .execute(&app.state.pool)
    .await
    .unwrap();
    let result = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "aktualni pisemnost".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(result.total, 2);
}

#[tokio::test]
async fn source_dates_sort_deterministically_and_unknown_dates_remain_unknown() {
    let app = App::new().await;
    document(&app, "Old source", None, Some("2019-03-01")).await;
    document(&app, "Recent source", None, Some("2026-09-24")).await;
    let native = document(&app, "Native publication", None, None).await;
    sqlx::query("UPDATE documents SET published_at='2026-09-25T12:00:00Z' WHERE id=$1")
        .bind(native)
        .execute(&app.state.pool)
        .await
        .unwrap();
    document(&app, "Unknown source date", None, None).await;
    for (sort, expected) in [
        (
            "newest",
            vec![
                "Native publication",
                "Recent source",
                "Old source",
                "Unknown source date",
            ],
        ),
        (
            "oldest",
            vec![
                "Old source",
                "Recent source",
                "Native publication",
                "Unknown source date",
            ],
        ),
    ] {
        let result = search::query(
            &app.state.pool,
            &SearchQuery {
                kind: "document".into(),
                sort: sort.into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            result
                .items
                .iter()
                .map(|h| h.title.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(result.items.last().unwrap().posted, "");
    }
}

#[tokio::test]
async fn pagination_reaches_old_records_without_loading_all_details() {
    let app = App::new().await;
    sqlx::query("INSERT INTO documents(title,status,created_at,source_published_on) SELECT 'Dokument '||n,'published','2026-11-01T10:00:00Z',DATE '2026-10-01' FROM generate_series(1,1025) n")
        .execute(&app.state.pool).await.unwrap();
    let q = scope("document", "all");
    let first = search::query(&app.state.pool, &q).await.unwrap();
    assert_eq!((first.total, first.items.len()), (1025, 20));
    let last = search::query(
        &app.state.pool,
        &SearchQuery {
            page: usize::MAX,
            ..q
        },
    )
    .await
    .unwrap();
    assert_eq!((last.page, last.items.len()), (52, 5));
    assert_eq!(last.items.last().unwrap().id, 1);
}

#[tokio::test]
async fn folded_literal_search_topic_changes_and_immediate_access_revocation() {
    let app = App::new().await;
    let id = document(
        &app,
        "ŽÁDOST o pronájem 100%",
        Some("published"),
        Some("2026-10-01"),
    )
    .await;
    posting(&app, id, false).await;
    for (query, total) in [("zadost prilis kun", 1), ("%", 1), ("_", 0), ("\\", 0)] {
        let found = search::query(
            &app.state.pool,
            &SearchQuery {
                q: query.into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(found.total, total);
    }
    sqlx::query("UPDATE documents SET category_id=1 WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE categories SET name='Tematický rozpočet' WHERE id=1")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let query = SearchQuery {
        q: "tematicky rozpocet".into(),
        ..Default::default()
    };
    assert_eq!(
        search::query(&app.state.pool, &query).await.unwrap().total,
        1
    );
    // Stored search text is deliberately unchanged by withdrawal. Access wins now.
    sqlx::query("UPDATE documents SET public_access=FALSE,access_blocked=TRUE WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        search::query(&app.state.pool, &query).await.unwrap().total,
        0
    );
    assert_eq!(
        search::query(&app.state.pool, &scope("notice", "current"))
            .await
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        app.call("GET", &format!("/api/v1/documents/{id}"), None, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn only_public_pages_are_searchable_and_filters_are_validated() {
    let app = App::new().await;
    document(&app, "Poplatky", None, None).await;
    sqlx::raw_sql("INSERT INTO documents(title,status,created_at) VALUES ('Poplatky tajné','draft','2026-09-29'); INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('poplatky','Poplatky','Úhrada poplatků',TRUE,'2026-09-29'),('tajne','Poplatky interní','Tajné',FALSE,'2026-09-29');")
        .execute(&app.state.pool).await.unwrap();
    let response = app
        .call("GET", "/api/v1/search?q=poplatky&state=all", None, false)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["total"], 2);
    assert_eq!(
        app.call("GET", "/api/v1/search?sort=bad", None, false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(
        search::query(
            &app.state.pool,
            &SearchQuery {
                q: "a ".repeat(11),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        search::query(
            &app.state.pool,
            &SearchQuery {
                kind: "page".into(),
                category: "Rozpočet".into(),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .total,
        0
    );
}

#[tokio::test]
async fn migrated_archive_search_never_reveals_previously_private_metadata() {
    let app = App::new().await;
    let id = document(
        &app,
        "Neveřejný původní název",
        Some("archived"),
        Some("2020-01-01"),
    )
    .await;
    posting(&app, id, true).await;
    sqlx::query(
        "UPDATE documents SET metadata_restricted=TRUE,public_title='Veřejný záznam' WHERE id=$1",
    )
    .bind(id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    for term in ["neveřejný", "původní", "žluťoučký"] {
        let result = search::query(
            &app.state.pool,
            &SearchQuery {
                q: term.into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result.total, 0);
    }
    let result = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "verejny zaznam".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.items[0].title, "Veřejný záznam");
    assert!(result.items[0].description.is_empty());
    let compact = obecni_web::model::notices::by_id(&app.state.pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(compact.title, "Veřejný záznam");
    assert!(compact.description.is_none());
    assert!(compact.reference_number.is_none());
    assert!(compact.issuer.is_none());
}
