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

// Explicit Czech mappings provide consistent results across database locales and preserve
// substring search semantics, including multiple words and literal '%' / '_'.
fn folded(expression: &str) -> String {
    let mut sql = expression.to_owned();
    for c in "ÁČĎÉĚÍŇÓŘŠŤÚŮÝŽáčďéěíňóřšťúůýž".chars() {
        sql = format!("replace({sql},'{c}','{}')", fold(&c.to_string()));
    }
    format!("lower({sql})")
}
fn build(q: &SearchQuery, day: time::Date, selection: &str) -> QueryBuilder<Postgres> {
    let mut b = QueryBuilder::new(
        "WITH notice_state AS (SELECT n.*, c.name AS category, status='published' AND published_on<=",
    );
    b.push_bind(day).push(" AND (withdraw_on IS NULL OR withdraw_on>").push_bind(day).push(") AS active FROM notices n LEFT JOIN categories c ON c.id=n.category_id WHERE status IN ('published','archived','withdrawn') AND published_on<=").push_bind(day).push("), public AS (
        SELECT id, 'notice' AS kind,
        CASE WHEN active THEN title ELSE coalesce(nullif(trim((review_json::jsonb->>'archive_title')),''),'Záznam úřední desky #'||id) END AS title,
        CASE WHEN active THEN coalesce(description,'') ELSE '' END AS description,
        CASE WHEN active THEN coalesce(reference_number,'')||' '||coalesce(issuer,'') ELSE '' END AS extra,
        '/uredni-deska/'||id AS path, coalesce(category,'Ostatní') AS category, published_on::text AS posted, NOT active AS archived
        FROM notice_state
        UNION ALL SELECT id,'document',title,coalesce(description,''),'','/dokumenty/'||id,'',coalesce(published_at,''),FALSE FROM documents WHERE status='published'
        UNION ALL SELECT id,'page',title,content,'','/stranky/'||slug,'',updated_at,FALSE FROM pages WHERE published=TRUE
        ) SELECT ").push(selection).push(" FROM public WHERE 1=1");
    if !q.kind.is_empty() && q.kind != "all" {
        b.push(" AND kind=").push_bind(q.kind.clone());
    }
    if q.state == "archive" {
        b.push(" AND archived=TRUE");
    } else if q.state != "all" {
        b.push(" AND archived=FALSE");
    }
    if !q.category.is_empty() {
        b.push(" AND category=").push_bind(q.category.clone());
    }
    let expression = folded("title||' '||description||' '||extra||' '||category");
    for word in fold(&q.q).split_whitespace() {
        b.push(" AND strpos(")
            .push(&expression)
            .push(",")
            .push_bind(word.to_owned())
            .push(")>0");
    }
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
            b.push(folded("title")).push(" ASC,kind,id");
        }
        "oldest" => {
            b.push("posted ASC,kind,id ASC");
        }
        _ => {
            b.push("posted DESC,kind,id DESC");
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
