//! Public search contracts shared by SSR, hydration and the static demo.
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

pub const PAGE_SIZE: usize = 20;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SearchQuery {
    pub q: String,
    pub kind: String,
    pub state: String,
    pub category: String,
    pub sort: String,
    pub page: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
pub struct Hit {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub description: String,
    pub path: String,
    pub category: String,
    pub posted: String,
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchPage {
    pub items: Vec<Hit>,
    pub total: usize,
    pub page: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoticePage {
    pub items: Vec<crate::content::Notice>,
    pub total: usize,
    pub page: usize,
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn search(query: SearchQuery) -> Result<SearchPage, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    crate::backend::search::query(&pool, &query)
        .await
        .map_err(|e| ServerFnError::new(e.1))
}
#[cfg(not(feature = "demo"))]
#[server]
pub async fn notices(query: SearchQuery) -> Result<NoticePage, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    let query = SearchQuery {
        kind: "notice".into(),
        ..query
    };
    let page = crate::backend::search::query(&pool, &query)
        .await
        .map_err(|e| ServerFnError::new(e.1))?;
    let mut items = Vec::with_capacity(page.items.len());
    for hit in page.items {
        if let Some(n) = crate::content::notice_from_pool(&pool, hit.id).await? {
            items.push(n);
        }
    }
    Ok(NoticePage {
        items,
        total: page.total,
        page: page.page,
    })
}
#[cfg(feature = "demo")]
pub async fn notices(query: SearchQuery) -> Result<NoticePage, ServerFnError> {
    let board = crate::content::load_board().await?;
    let mut items: Vec<_> = board
        .notices
        .into_iter()
        .filter(|n| {
            (query.state == "all" || n.archived == (query.state == "archive"))
                && n.matches(&query.q)
                && (query.category.is_empty() || n.category == query.category)
        })
        .collect();
    match query.sort.as_str() {
        "title" => items.sort_by_key(|n| (crate::content::fold(&n.title), n.id)),
        "oldest" => items.sort_by_key(|n| (n.posted_iso.clone(), n.id)),
        _ => items.sort_by_key(|n| std::cmp::Reverse((n.posted_iso.clone(), n.id))),
    }
    let total = items.len();
    let page = query.page.max(1).min(total.div_ceil(PAGE_SIZE).max(1));
    let items = items
        .into_iter()
        .skip((page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();
    Ok(NoticePage { items, total, page })
}
#[cfg(feature = "demo")]
pub async fn search(query: SearchQuery) -> Result<SearchPage, ServerFnError> {
    let page = notices(query).await?;
    Ok(SearchPage {
        items: page
            .items
            .into_iter()
            .map(|n| Hit {
                id: n.id,
                path: n.href(),
                kind: "notice".into(),
                title: n.title,
                description: n.description,
                category: n.category,
                posted: n.posted_iso,
                archived: n.archived,
            })
            .collect(),
        total: page.total,
        page: page.page,
    })
}
