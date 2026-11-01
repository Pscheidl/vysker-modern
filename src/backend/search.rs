//! Search only the current public projection, before counting or pagination.
use super::{Backend, Result, bad};
use crate::{
    content::fold,
    search::{Hit, PAGE_SIZE, SearchPage, SearchQuery},
};
use axum::{
    Json,
    extract::{Query, State},
};
use sqlx::{PgPool, Postgres, QueryBuilder};

/// Escape LIKE metacharacters so user input keeps literal substring semantics.
fn literal_pattern(word: &str) -> String {
    format!(
        "%{}%",
        word.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
fn build(q: &SearchQuery, day: time::Date, selection: &str) -> QueryBuilder<Postgres> {
    let mut b = QueryBuilder::new("WITH public AS (");
    let documents = q.kind != "page";
    let pages = !["notice", "document"].contains(&q.kind.as_str())
        && q.state != "archive"
        && q.category.is_empty();
    if documents {
        let date = if q.kind == "notice" {
            "d.published_on"
        } else {
            "d.catalog_date"
        };
        b.push(format!("SELECT d.id,CASE WHEN d.board_status IS NULL THEN 'document' ELSE 'notice' END AS kind,CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Záznam úřední desky #'||d.id) ELSE d.title END AS title,CASE WHEN d.metadata_restricted THEN '' ELSE substr(d.description,1,400) END AS description,'/dokumenty/'||d.id AS path,coalesce(c.name,'Ostatní') AS category,coalesce({date}::text,'') AS posted,{date} AS sort_date,coalesce(d.board_status IN ('withdrawn','archived'),FALSE) AS archived FROM documents d LEFT JOIN categories c ON c.id=d.category_id WHERE document_is_public(d,"))
            .push_bind(day).push(")");
        if q.kind == "notice" {
            if q.state == "archive" {
                b.push(" AND d.board_status IN ('withdrawn','archived') AND d.has_removed_posting");
            } else if q.state == "all" {
                b.push(" AND d.board_status IN ('published','withdrawn','archived') AND (d.has_current_posting OR d.has_removed_posting)");
            } else {
                b.push(" AND d.board_status='published' AND d.has_current_posting");
            }
        } else if q.state == "archive" {
            b.push(" AND d.board_status IN ('withdrawn','archived') AND d.has_removed_posting");
        }
        if !q.category.is_empty() {
            b.push(" AND c.name=").push_bind(q.category.clone());
        }
        for word in fold(&q.q).split_whitespace() {
            b.push(" AND d.search_text LIKE ")
                .push_bind(literal_pattern(word));
        }
    }
    if pages {
        if documents {
            b.push(" UNION ALL ");
        }
        b.push("SELECT id,'page',title,substr(content,1,400),'/stranky/'||slug,'',updated_at,updated_at::date,FALSE FROM pages WHERE published=TRUE");
        for word in fold(&q.q).split_whitespace() {
            b.push(" AND document_fold(title||' '||content) LIKE ")
                .push_bind(literal_pattern(word));
        }
    }
    if !documents && !pages {
        b.push("SELECT 0::bigint AS id,''::text AS kind,''::text AS title,''::text AS description,''::text AS path,''::text AS category,''::text AS posted,NULL::date AS sort_date,FALSE AS archived WHERE FALSE");
    }
    b.push(") SELECT ").push(selection).push(" FROM public");
    b
}

pub async fn query(pool: &PgPool, q: &SearchQuery) -> Result<SearchPage> {
    if q.q.len() > 800
        || q.q.chars().count() > 200
        || q.q.split_whitespace().count() > 10
        || q.category.len() > 200
        || !["", "all", "notice", "document", "page"].contains(&q.kind.as_str())
        || !["", "current", "archive", "all"].contains(&q.state.as_str())
        || !["", "newest", "oldest", "title"].contains(&q.sort.as_str())
    {
        return Err(bad("Zadejte nejvýše 200 znaků a 10 slov a platné filtry."));
    }
    let day = super::today(time::OffsetDateTime::now_utc());
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await?;
    let total: i64 = build(q, day, "count(*)")
        .build_query_scalar()
        .fetch_one(&mut *tx)
        .await?;
    let page = q
        .page
        .max(1)
        .min((total as usize).div_ceil(PAGE_SIZE).max(1));
    let mut b = build(
        q,
        day,
        "id,kind,title,substr(description,1,400) AS description,path,category,posted,archived",
    );
    b.push(" ORDER BY ");
    match q.sort.as_str() {
        "title" => {
            b.push("document_fold(title) ASC,kind,id");
        }
        "oldest" => {
            b.push("sort_date ASC NULLS LAST,id ASC,kind");
        }
        _ => {
            b.push("sort_date DESC NULLS LAST,id DESC,kind");
        }
    }
    b.push(" LIMIT ")
        .push_bind(PAGE_SIZE as i64)
        .push(" OFFSET ")
        .push_bind(((page - 1) * PAGE_SIZE) as i64);
    let items: Vec<Hit> = b.build_query_as().fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(SearchPage {
        items,
        total: total as usize,
        page,
    })
}
pub async fn endpoint(
    State(s): State<Backend>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<SearchPage>> {
    Ok(Json(query(&s.pool, &q).await?))
}
