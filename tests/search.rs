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
    sqlx::query("INSERT INTO document_postings(document_id,origin,certainty,initial_state,started_on,removed_on,after_action,snapshot_sha256) SELECT id,'source','documented',CASE WHEN $2 THEN 'historical' ELSE 'current' END,published_on,CASE WHEN $2 THEN DATE '2026-10-01' ELSE NULL END,'retain',document_fingerprint(id) FROM documents WHERE id=$1")
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
async fn page_search_previews_format_complete_markdown_without_changing_plain_descriptions() {
    let app = App::new().await;
    let text = "ž".repeat(600);
    let content = format!("**{text}**\n\n[Kontakt][kontakt]\n\n[kontakt]: /obec");
    sqlx::query("INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('nahled','Náhled',$1,TRUE,'2026-11-01')")
        .bind(&content)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let id = document(&app, "Popis", None, Some("2026-11-01")).await;
    sqlx::query("UPDATE documents SET description='**Doslovný popis**' WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let response = app
        .call("GET", "/api/v1/search?state=all", None, false)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let items = body["items"].as_array().unwrap();
    let page = items.iter().find(|hit| hit["kind"] == "page").unwrap();
    assert_eq!(page["description"].as_str().unwrap().chars().count(), 400);
    let preview = page["preview_html"].as_str().unwrap();
    assert!(preview.contains(&format!("<strong>{text}</strong>")));
    assert!(preview.contains("<p>Kontakt</p>"));
    assert!(!preview.contains("<a "));
    let document = items.iter().find(|hit| hit["kind"] == "document").unwrap();
    assert_eq!(document["description"], "**Doslovný popis**");
    assert!(document["preview_html"].is_null());
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
    let pages = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "poplatky".into(),
            kind: "page".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(pages.total, 1);
    assert_eq!(pages.items[0].path, "/stranky/poplatky");
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

#[tokio::test]
async fn shared_archive_contains_ordinary_and_removed_documents_but_no_active_or_uncertain_posting()
{
    let app = App::new().await;
    let ordinary = document(&app, "Archivovaný běžný dokument", None, Some("2020-01-01")).await;
    sqlx::query("UPDATE documents SET status='archived' WHERE id=$1")
        .bind(ordinary)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let removed = document(
        &app,
        "Sejmutý dokument",
        Some("archived"),
        Some("2020-02-01"),
    )
    .await;
    posting(&app, removed, true).await;
    let current = document(
        &app,
        "Znovu vyvěšený dokument",
        Some("published"),
        Some("2020-03-01"),
    )
    .await;
    posting(&app, current, true).await;
    posting(&app, current, false).await;
    let scheduled = document(
        &app,
        "Plánované opětovné vyvěšení",
        Some("archived"),
        Some("2020-04-01"),
    )
    .await;
    posting(&app, scheduled, true).await;
    sqlx::query("UPDATE documents SET board_status='scheduled' WHERE id=$1")
        .bind(scheduled)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let uncertain = document(
        &app,
        "Neověřené sejmutí",
        Some("archived"),
        Some("2020-05-01"),
    )
    .await;
    sqlx::query("UPDATE documents SET status='archived',lock_state='review' WHERE id=$1")
        .bind(uncertain)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let private = document(&app, "Soukromý archiv", None, Some("2020-06-01")).await;
    sqlx::query("UPDATE documents SET status='archived',public_access=FALSE,access_blocked=TRUE WHERE id=$1").bind(private).execute(&app.state.pool).await.unwrap();
    let archive = search::query(&app.state.pool, &scope("document", "archive"))
        .await
        .unwrap();
    assert_eq!(archive.total, 2);
    assert_eq!(
        archive.items.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [removed, ordinary]
    );
    assert!(archive.items.iter().all(|hit| hit.archived));
    let board_archive = search::query(&app.state.pool, &scope("notice", "archive"))
        .await
        .unwrap();
    assert_eq!(
        board_archive
            .items
            .iter()
            .map(|hit| hit.id)
            .collect::<Vec<_>>(),
        [removed]
    );
    let catalogue = search::query(&app.state.pool, &scope("document", "all"))
        .await
        .unwrap();
    assert_eq!(catalogue.total, 5);
    assert_eq!(
        catalogue
            .items
            .iter()
            .filter(|hit| hit.id == current)
            .count(),
        1
    );
    assert!(
        !catalogue
            .items
            .iter()
            .find(|hit| hit.id == current)
            .unwrap()
            .archived
    );
    for id in [scheduled, uncertain] {
        let hit = catalogue.items.iter().find(|hit| hit.id == id).unwrap();
        assert_eq!(hit.kind, "document");
        assert!(!hit.archived);
    }
}

#[tokio::test]
async fn detail_keeps_creation_date_distinct_from_source_publication_and_import() {
    let app = App::new().await;
    let id = document(&app, "Rozlišená data", None, Some("2020-03-04")).await;
    sqlx::query("UPDATE documents SET document_date='2020-02-01' WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let detail = obecni_web::catalog::document_from_pool(&app.state.pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.document_date.as_deref(), Some("2020-02-01"));
    assert_eq!(detail.source_published_on.as_deref(), Some("2020-03-04"));
    let local = document(&app, "Místní zveřejnění před půlnocí UTC", None, None).await;
    sqlx::query("UPDATE documents SET published_at='2020-03-04T23:30:00Z' WHERE id=$1")
        .bind(local)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let catalogue_day: String =
        sqlx::query_scalar("SELECT catalog_date::text FROM documents WHERE id=$1")
            .bind(local)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(catalogue_day, "2020-03-05");
    let unknown = document(
        &app,
        "Datum vytvoření není doloženo",
        None,
        Some("2019-05-06"),
    )
    .await;
    let detail = obecni_web::catalog::document_from_pool(&app.state.pool, unknown)
        .await
        .unwrap()
        .unwrap();
    assert!(detail.document_date.is_none());
    assert_eq!(detail.source_published_on.as_deref(), Some("2019-05-06"));
}

async fn indexed_attachment(app: &App, document_id: i64, name: &str, pages: &[&str]) -> i64 {
    let id: i64 = sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES ($1,$2,'application/pdf',8,$3) RETURNING id")
        .bind(document_id).bind(name).bind(b"%PDF-1.4".to_vec())
        .fetch_one(&app.state.pool).await.unwrap();
    sqlx::query("INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,method) SELECT id,sha256,name,$2,'ready','text' FROM attachments WHERE id=$1")
        .bind(id).bind(obecni_web::backend::attachment_index::EXTRACTOR_VERSION)
        .execute(&app.state.pool).await.unwrap();
    for (index, content) in pages.iter().enumerate() {
        sqlx::query("INSERT INTO attachment_search_pages(attachment_id,page,label,content) VALUES ($1,$2,$3,$4)")
            .bind(id).bind((index+1) as i32).bind(format!("Strana {}", index+1)).bind(content)
            .execute(&app.state.pool).await.unwrap();
    }
    id
}

async fn find(app: &App, term: &str) -> obecni_web::search::SearchPage {
    search::query(
        &app.state.pool,
        &SearchQuery {
            q: term.into(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn relevance_prefers_titles_then_metadata_then_attachment_text_and_honors_explicit_sort() {
    let app = App::new().await;
    let exact = document(&app, "Pronájem hospody", None, Some("2010-01-01")).await;
    let title = document(
        &app,
        "Záměr na pronájem obecní hospody",
        None,
        Some("2011-01-01"),
    )
    .await;
    let partial = document(&app, "Pronájem obecního majetku", None, Some("2012-01-01")).await;
    indexed_attachment(&app, partial, "zapis.pdf", &["Projednání hospody."]).await;
    let metadata = document(&app, "Záměr obce", None, Some("2020-01-01")).await;
    sqlx::query("UPDATE documents SET description='Pronájem hospody' WHERE id=$1")
        .bind(metadata)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let body = document(&app, "Zápis ze zasedání", None, Some("2026-01-01")).await;
    indexed_attachment(
        &app,
        body,
        "zapis.pdf",
        &["Pronájem hospody. Pronájem hospody. Pronájem hospody."],
    )
    .await;
    let found = find(&app, "pronajem hospody").await;
    assert_eq!(
        found.items.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [exact, title, partial, metadata, body]
    );
    let newest = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "pronajem hospody".into(),
            sort: "newest".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(newest.items[0].id, body);
    assert_eq!(newest.items.last().unwrap().id, exact);
    let alpha = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "pronajem hospody".into(),
            sort: "title".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(alpha.items[0].id, exact);
    let explicit = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "pronajem hospody".into(),
            sort: "relevance".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(explicit.items[0].id, exact);
    sqlx::query("INSERT INTO pages(slug,title,content,published,updated_at) VALUES ('pronajem','Pronájem hospody','Veřejné informace',TRUE,'2000-01-01')").execute(&app.state.pool).await.unwrap();
    let found = find(&app, "pronajem hospody").await;
    assert_eq!(found.items[1].kind, "page");
    assert_eq!(found.items.last().unwrap().id, body);
}

#[tokio::test]
async fn attachment_words_can_span_title_and_pages_without_duplicate_results() {
    let app = App::new().await;
    let id = document(&app, "Rozpočet", None, None).await;
    let attachment = indexed_attachment(
        &app,
        id,
        "rozpocet.pdf",
        &[
            "Oprava hospody.",
            "Položka kanalizace.",
            "Hospody a kanalizace společně.",
        ],
    )
    .await;
    indexed_attachment(&app, id, "priloha.pdf", &["Oprava hospody a kanalizace."]).await;
    let found = find(&app, "rozpocet hospody kanalizace").await;
    assert_eq!(found.total, 1);
    let passage = found.items[0].attachment.as_ref().unwrap();
    assert_eq!((passage.id, passage.page), (attachment, 3));
    assert_eq!(passage.label, "Strana 3");
    assert!(passage.excerpt.contains("Hospody"));
    let spanning = find(&app, "oprava polozka").await;
    assert_eq!(spanning.total, 1);
    assert_eq!(find(&app, "rozpocet chybejici").await.total, 0);
}

#[tokio::test]
async fn attachment_snippets_are_current_unicode_safe_literal_text_centered_on_the_match() {
    let app = App::new().await;
    let id = document(&app, "Zápis", None, None).await;
    let content = format!(
        "{} Nalezený žluťoučký 100% pod_tržítko \\ <script>alert(1)</script> {}",
        "Úvod ".repeat(120),
        "Konec ".repeat(100)
    );
    indexed_attachment(&app, id, "zapisy.pdf", &[&content]).await;
    for query in ["ZLUTOUCKY", "%", "_", "\\"] {
        let found = find(&app, query).await;
        assert_eq!(found.total, 1, "{query}");
        let passage = found.items[0].attachment.as_ref().unwrap();
        assert!(passage.excerpt.starts_with('…'));
        assert!(passage.excerpt.ends_with('…'));
        assert!(passage.excerpt.contains("žluťoučký"));
        assert!(passage.excerpt.contains("<script>alert(1)</script>"));
        assert!(found.items[0].preview_html.is_none());
    }
    assert_eq!(find(&app, "podxtrzitko").await.total, 0);
}

#[tokio::test]
async fn attachment_search_uses_exact_download_visibility_before_counting() {
    let app = App::new().await;
    let public = document(&app, "Veřejný", None, None).await;
    let hidden_attachment = document(&app, "Skrytá příloha", None, None).await;
    let hidden_file =
        indexed_attachment(&app, hidden_attachment, "skryta.pdf", &["tajemstvi"]).await;
    sqlx::query("UPDATE attachments SET is_public=FALSE WHERE id=$1")
        .bind(hidden_file)
        .execute(&app.state.pool)
        .await
        .unwrap();
    // Even an old index forcibly retained by an operator must not override live permissions.
    indexed_attachment(&app, public, "verejna.pdf", &["tajemstvi"]).await;
    let hidden_document = document(&app, "Neveřejný", None, None).await;
    indexed_attachment(&app, hidden_document, "interni.pdf", &["tajemstvi"]).await;
    sqlx::query("UPDATE documents SET public_access=FALSE,access_blocked=TRUE WHERE id=$1")
        .bind(hidden_document)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let draft = document(&app, "Koncept", None, None).await;
    indexed_attachment(&app, draft, "koncept.pdf", &["tajemstvi"]).await;
    sqlx::query("UPDATE documents SET status='draft' WHERE id=$1")
        .bind(draft)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let future = document(&app, "Budoucí", None, Some("9998-01-01")).await;
    indexed_attachment(&app, future, "budouci.pdf", &["tajemstvi"]).await;
    sqlx::query("UPDATE documents SET board_status='scheduled',public_access=FALSE WHERE id=$1")
        .bind(future)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let expired = document(&app, "Uplynulá dostupnost", None, None).await;
    indexed_attachment(&app, expired, "uplynule.pdf", &["tajemstvi"]).await;
    sqlx::query("UPDATE documents SET public_until='2000-01-01' WHERE id=$1")
        .bind(expired)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let retained = document(
        &app,
        "Ponechaný archiv",
        Some("archived"),
        Some("2020-01-01"),
    )
    .await;
    indexed_attachment(&app, retained, "archiv.pdf", &["tajemstvi"]).await;
    posting(&app, retained, true).await;
    let expired_archive = document(
        &app,
        "Archiv po lhůtě",
        Some("withdrawn"),
        Some("2020-01-01"),
    )
    .await;
    indexed_attachment(&app, expired_archive, "archiv.pdf", &["tajemstvi"]).await;
    posting(&app, expired_archive, true).await;
    sqlx::query("UPDATE documents SET posting_public_until='2000-01-01' WHERE id=$1")
        .bind(expired_archive)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let found = find(&app, "tajemstvi").await;
    assert_eq!(found.total, 2);
    assert!(found.items.iter().any(|hit| hit.id == public));
    assert!(found.items.iter().any(|hit| hit.id == retained));
    sqlx::query("UPDATE documents SET attachments_public=FALSE WHERE id=$1")
        .bind(public)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(find(&app, "tajemstvi").await.total, 1);
    let title = find(&app, "verejny").await;
    assert_eq!(title.total, 1);
    assert!(title.items[0].attachment.is_none());
    let archive = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "tajemstvi".into(),
            kind: "notice".into(),
            state: "archive".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(archive.total, 1);
    assert_eq!(archive.items[0].id, retained);
}

#[tokio::test]
async fn stale_failed_and_replaced_extractions_never_match() {
    let app = App::new().await;
    for (statement, value) in [
        (
            "UPDATE attachment_extractions SET status=$2 WHERE attachment_id=$1",
            "failed",
        ),
        (
            "UPDATE attachment_extractions SET source_name=$2 WHERE attachment_id=$1",
            "old.pdf",
        ),
    ] {
        let id = document(&app, "Záznam", None, None).await;
        let a = indexed_attachment(&app, id, "soubor.pdf", &["nezverejnovat"]).await;
        sqlx::query(statement)
            .bind(a)
            .bind(value)
            .execute(&app.state.pool)
            .await
            .unwrap();
    }
    let id = document(&app, "Stará verze", None, None).await;
    let a = indexed_attachment(&app, id, "soubor.pdf", &["nezverejnovat"]).await;
    sqlx::query("UPDATE attachment_extractions SET extractor_version=0 WHERE attachment_id=$1")
        .bind(a)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(find(&app, "nezverejnovat").await.total, 0);
    for statement in [
        "UPDATE attachments SET data=convert_to('new version','UTF8') WHERE id=$1",
        "UPDATE attachments SET name='new.pdf' WHERE id=$1",
        "UPDATE attachments SET data=NULL,removed_at='2026-11-01' WHERE id=$1",
    ] {
        let id = document(&app, "Nahrazená příloha", None, None).await;
        let a = indexed_attachment(&app, id, "soubor.pdf", &["staryobsah"]).await;
        assert_eq!(find(&app, "staryobsah").await.total, 1);
        sqlx::query(statement)
            .bind(a)
            .execute(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(find(&app, "staryobsah").await.total, 0);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM attachment_search_pages WHERE attachment_id=$1",
        )
        .bind(a)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn attachment_matches_are_deduplicated_before_pagination_and_filters() {
    let app = App::new().await;
    for n in 0..23 {
        let id = document(&app, &format!("Zápis {n}"), None, Some("2026-01-01")).await;
        indexed_attachment(&app, id, "zapis.pdf", &["spolecnyvyraz", "spolecnyvyraz"]).await;
        indexed_attachment(&app, id, "usneseni.pdf", &["spolecnyvyraz"]).await;
    }
    let first = find(&app, "spolecnyvyraz").await;
    assert_eq!((first.total, first.items.len()), (23, 20));
    let last = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "spolecnyvyraz".into(),
            page: 2,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((last.total, last.items.len()), (23, 3));
    assert!(
        last.items
            .iter()
            .all(|hit| !first.items.iter().any(|old| old.id == hit.id))
    );
    let missing = search::query(
        &app.state.pool,
        &SearchQuery {
            q: "spolecnyvyraz".into(),
            category: "Neexistující".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(missing.total, 0);
}

#[tokio::test]
async fn filenames_are_immediate_ranked_matches_with_the_same_public_download_rules() {
    let app = App::new().await;
    let title = document(&app, "Pronájem", None, Some("2010-01-01")).await;
    let body = document(&app, "Obecní zápis", None, Some("2026-10-01")).await;
    indexed_attachment(&app, body, "zapis.pdf", &["Pronájem"]).await;
    for status in [None, Some("processing"), Some("failed")] {
        let id = document(&app, "Podklady k jednání", None, Some("2015-01-01")).await;
        let a = indexed_attachment(&app, id, "Pronájem_100%.pdf", &["Původní text"]).await;
        if let Some(status) = status {
            sqlx::query("UPDATE attachment_extractions SET status=$2 WHERE attachment_id=$1")
                .bind(a)
                .bind(status)
                .execute(&app.state.pool)
                .await
                .unwrap();
        } else {
            sqlx::query("DELETE FROM attachment_extractions WHERE attachment_id=$1")
                .bind(a)
                .execute(&app.state.pool)
                .await
                .unwrap();
        }
        let specific = find(&app, "pronajem_100%").await;
        let hit = specific.items.iter().find(|hit| hit.id == id).unwrap();
        let attachment = hit.attachment.as_ref().unwrap();
        assert_eq!(attachment.id, a);
        assert_eq!(attachment.name, "Pronájem_100%.pdf");
        assert_eq!(attachment.page, 0);
        assert!(attachment.excerpt.is_empty());
        assert_eq!(find(&app, "pronajemx100").await.total, 0);
    }
    let ranked = find(&app, "pronajem").await;
    assert_eq!(ranked.total, 5);
    assert_eq!(ranked.items[0].id, title);
    assert_eq!(ranked.items.last().unwrap().id, body);
    assert_eq!(find(&app, "podklady pronajem").await.total, 3);

    for restriction in [
        "UPDATE attachments SET is_public=FALSE WHERE id=$1",
        "UPDATE attachments SET data=NULL,removed_at='2026-11-01' WHERE id=$1",
        "UPDATE documents SET attachments_public=FALSE WHERE id=(SELECT document_id FROM attachments WHERE id=$1)",
        "UPDATE documents SET public_access=FALSE WHERE id=(SELECT document_id FROM attachments WHERE id=$1)",
        "UPDATE documents SET public_access=FALSE,access_blocked=TRUE WHERE id=(SELECT document_id FROM attachments WHERE id=$1)",
    ] {
        let id = document(&app, "Soukromé podklady", None, None).await;
        let a = indexed_attachment(&app, id, "skrytynazev.pdf", &["Původní text"]).await;
        sqlx::query(restriction)
            .bind(a)
            .execute(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(find(&app, "skrytynazev").await.total, 0);
    }
    let id = document(&app, "Opravené podklady", None, None).await;
    let a = indexed_attachment(&app, id, "starynazev.pdf", &["Původní text"]).await;
    sqlx::query("UPDATE attachments SET name='novynazev.pdf' WHERE id=$1")
        .bind(a)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(find(&app, "starynazev").await.total, 0);
    assert_eq!(find(&app, "novynazev").await.total, 1);
}
