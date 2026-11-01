//! Search only the current public projection, before counting or pagination.
use super::{Backend, Result, bad};
use crate::{
    content::fold,
    search::{AttachmentMatch, Hit, PAGE_SIZE, SearchPage, SearchQuery},
};
use axum::{
    Json,
    extract::{Query, State},
};
use sqlx::{PgPool, Postgres, QueryBuilder};

const ARCHIVED: &str = "((d.board_status IS NULL AND d.status='archived') OR d.has_removed_posting) AND NOT d.has_current_posting AND d.board_status IS DISTINCT FROM 'scheduled'";

/// Escape LIKE metacharacters so user input keeps literal substring semantics.
fn literal_pattern(word: &str) -> String {
    format!(
        "%{}%",
        word.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
fn words(q: &SearchQuery) -> Vec<String> {
    fold(&q.q).split_whitespace().map(str::to_owned).collect()
}

/// Compare the current public title first, then metadata, then attachment content.
/// Each category has a disjoint score range, so frequent body mentions cannot
/// outrank a title match.
fn relevance(
    b: &mut QueryBuilder<Postgres>,
    title: &str,
    metadata: Option<&str>,
    filename_mask: Option<&str>,
    words: &[String],
    phrase: &str,
) {
    if words.is_empty() {
        b.push("0");
        return;
    }
    b.push("CASE WHEN document_fold(")
        .push(title)
        .push(")=")
        .push_bind(phrase.to_owned())
        .push(" THEN 0 WHEN ");
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            b.push(" AND ");
        }
        b.push("document_fold(")
            .push(title)
            .push(") LIKE ")
            .push_bind(literal_pattern(word));
    }
    b.push(" THEN 1 WHEN (");
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            b.push(" OR ");
        }
        b.push("document_fold(")
            .push(title)
            .push(") LIKE ")
            .push_bind(literal_pattern(word));
    }
    b.push(") THEN 2 ");
    if let Some(filename_mask) = filename_mask {
        b.push("WHEN ").push(filename_mask).push("<>0 THEN 3 ");
    }
    if let Some(metadata) = metadata {
        b.push("WHEN ");
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                b.push(" AND ");
            }
            b.push(metadata)
                .push(" LIKE ")
                .push_bind(literal_pattern(word));
        }
        b.push(" THEN 4 ");
    }
    b.push("ELSE 5 END");
}

/// Match the same live visibility rules as the original attachment download.
fn public_file_predicate(b: &mut QueryBuilder<Postgres>, day: time::Date) {
    b.push(
        "a.data IS NOT NULL AND a.removed_at IS NULL AND a.is_public AND document_files_public(d,",
    )
    .push_bind(day)
    .push(")");
}
/// The invalidation trigger removes extracted text immediately on replacement.
fn ready_extraction_predicate(b: &mut QueryBuilder<Postgres>) {
    b.push("e.status='ready' AND e.source_name=a.name AND e.extractor_version=")
        .push_bind(super::attachment_index::EXTRACTOR_VERSION);
}
fn any_word(b: &mut QueryBuilder<Postgres>, column: &str, words: &[String]) {
    b.push("(");
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            b.push(" OR ");
        }
        b.push(column)
            .push(" LIKE ")
            .push_bind(literal_pattern(word));
    }
    b.push(")");
}
fn word_mask(b: &mut QueryBuilder<Postgres>, column: &str, words: &[String]) {
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            b.push("+");
        }
        b.push("CASE WHEN ")
            .push(column)
            .push(" LIKE ")
            .push_bind(literal_pattern(word))
            .push(" THEN ")
            .push_bind(1_i32 << i)
            .push(" ELSE 0 END");
    }
}
fn build(q: &SearchQuery, day: time::Date, selection: &str) -> QueryBuilder<Postgres> {
    let words = words(q);
    let phrase = words.join(" ");
    let documents = q.kind != "page";
    let pages = !["notice", "document"].contains(&q.kind.as_str())
        && q.state != "archive"
        && q.category.is_empty();
    let attachments = documents && !words.is_empty();
    let mut b = QueryBuilder::new("WITH ");
    if attachments {
        // Aggregate one bit per word across all filenames and extracted pages.
        // Filenames are searchable immediately, independently of extraction state.
        b.push("attachment_matches AS (SELECT document_id,bit_or(mask) AS mask,bit_or(name_mask) AS name_mask FROM (SELECT a.document_id,");
        word_mask(&mut b, "p.search_text", &words);
        b.push(" AS mask,0 AS name_mask FROM attachment_search_pages p JOIN attachments a ON a.id=p.attachment_id JOIN attachment_extractions e ON e.attachment_id=a.id JOIN documents d ON d.id=a.document_id WHERE ");
        public_file_predicate(&mut b, day);
        b.push(" AND ");
        ready_extraction_predicate(&mut b);
        b.push(" AND ");
        any_word(&mut b, "p.search_text", &words);
        b.push(
            " UNION ALL SELECT document_id,name_mask AS mask,name_mask FROM (SELECT a.document_id,",
        );
        word_mask(&mut b, "document_fold(a.name)", &words);
        b.push(" AS name_mask FROM attachments a JOIN documents d ON d.id=a.document_id WHERE ");
        public_file_predicate(&mut b, day);
        b.push(" AND ");
        any_word(&mut b, "document_fold(a.name)", &words);
        b.push(") filenames) matches GROUP BY document_id), ");
    }
    b.push("public AS (");
    if documents {
        let date = if q.kind == "notice" {
            "d.published_on"
        } else {
            "d.catalog_date"
        };
        let title = "CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Záznam úřední desky #'||d.id) ELSE d.title END";
        b.push(format!("SELECT d.id,CASE WHEN (d.board_status='published' AND d.has_current_posting) OR (d.has_removed_posting AND NOT d.has_current_posting AND d.board_status IS DISTINCT FROM 'scheduled') THEN 'notice' ELSE 'document' END AS kind,{title} AS title,CASE WHEN d.metadata_restricted THEN '' ELSE substr(d.description,1,400) END AS description,'/dokumenty/'||d.id AS path,coalesce(c.name,'Ostatní') AS category,coalesce({date}::text,'') AS posted,{date} AS sort_date,({ARCHIVED}) AS archived,"));
        relevance(
            &mut b,
            title,
            Some("d.search_text"),
            attachments.then_some("coalesce(am.name_mask,0)"),
            &words,
            &phrase,
        );
        b.push(" AS relevance FROM documents d LEFT JOIN categories c ON c.id=d.category_id");
        if attachments {
            b.push(" LEFT JOIN attachment_matches am ON am.document_id=d.id");
        }
        b.push(" WHERE document_is_public(d,")
            .push_bind(day)
            .push(")");
        if q.kind == "notice" {
            if q.state == "archive" {
                b.push(" AND d.has_removed_posting AND NOT d.has_current_posting AND d.board_status IS DISTINCT FROM 'scheduled'");
            } else if q.state == "all" {
                b.push(" AND d.board_status IN ('published','withdrawn','archived') AND (d.has_current_posting OR d.has_removed_posting)");
            } else {
                b.push(" AND d.board_status='published' AND d.has_current_posting");
            }
        } else if q.state == "archive" {
            b.push(" AND (").push(ARCHIVED).push(")");
        }
        if !q.category.is_empty() {
            b.push(" AND c.name=").push_bind(q.category.clone());
        }
        for (i, word) in words.iter().enumerate() {
            b.push(" AND (d.search_text LIKE ")
                .push_bind(literal_pattern(word))
                .push(" OR (coalesce(am.mask,0) & ")
                .push_bind(1_i32 << i)
                .push(")<>0)");
        }
    }
    if pages {
        if documents {
            b.push(" UNION ALL ");
        }
        b.push("SELECT id,'page' AS kind,title,content AS description,'/stranky/'||slug AS path,''::text AS category,updated_at AS posted,updated_at::date AS sort_date,FALSE AS archived,");
        relevance(&mut b, "title", None, None, &words, &phrase);
        b.push(" AS relevance FROM pages WHERE published=TRUE");
        for word in &words {
            b.push(" AND document_fold(title||' '||content) LIKE ")
                .push_bind(literal_pattern(word));
        }
    }
    if !documents && !pages {
        b.push("SELECT 0::bigint AS id,''::text AS kind,''::text AS title,''::text AS description,''::text AS path,''::text AS category,''::text AS posted,NULL::date AS sort_date,FALSE AS archived,0 AS relevance WHERE FALSE");
    }
    b.push(") SELECT ").push(selection).push(" FROM public");
    b
}

#[derive(sqlx::FromRow)]
struct Passage {
    document_id: i64,
    id: i64,
    name: String,
    page: i32,
    label: String,
    excerpt: String,
}
async fn passages(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    items: &mut [Hit],
    q: &SearchQuery,
    day: time::Date,
) -> Result<()> {
    let words = words(q);
    let ids: Vec<_> = items
        .iter()
        .filter(|hit| hit.kind != "page")
        .map(|hit| hit.id)
        .collect();
    if words.is_empty() || ids.is_empty() {
        return Ok(());
    }
    let mut b = QueryBuilder::new(
        "SELECT DISTINCT ON (a.document_id) a.document_id,a.id,a.name,coalesce(p.page,0) AS page,coalesce(p.label,'') AS label,CASE WHEN p.content IS NULL THEN '' ELSE CASE WHEN match_position.first_match>81 THEN '…' ELSE '' END || substr(p.content,greatest(match_position.first_match-80,1),380) || CASE WHEN length(p.content)>greatest(match_position.first_match-80,1)+379 THEN '…' ELSE '' END END AS excerpt FROM attachments a JOIN documents d ON d.id=a.document_id LEFT JOIN attachment_extractions e ON e.attachment_id=a.id AND ",
    );
    ready_extraction_predicate(&mut b);
    b.push(" LEFT JOIN attachment_search_pages p ON p.attachment_id=e.attachment_id CROSS JOIN LATERAL (SELECT coalesce(least(");
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            b.push(",");
        }
        b.push("nullif(strpos(p.search_text,")
            .push_bind(word.clone())
            .push("),0)");
    }
    b.push("),1) AS first_match) match_position WHERE a.document_id=ANY(")
        .push_bind(ids)
        .push(") AND ");
    public_file_predicate(&mut b, day);
    b.push(" AND (");
    any_word(&mut b, "p.search_text", &words);
    b.push(" OR ");
    any_word(&mut b, "document_fold(a.name)", &words);
    b.push(") ORDER BY a.document_id,CASE WHEN ");
    any_word(&mut b, "document_fold(a.name)", &words);
    b.push(" THEN 0 ELSE 1 END,(");
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            b.push("+");
        }
        b.push("CASE WHEN p.search_text LIKE ")
            .push_bind(literal_pattern(word))
            .push(" THEN 1 ELSE 0 END");
    }
    b.push(") DESC,a.sort_order,a.id,p.page");
    for passage in b.build_query_as::<Passage>().fetch_all(&mut **tx).await? {
        if let Some(hit) = items
            .iter_mut()
            .find(|hit| hit.kind != "page" && hit.id == passage.document_id)
        {
            hit.attachment = Some(AttachmentMatch {
                id: passage.id,
                name: passage.name,
                page: passage.page,
                label: passage.label,
                excerpt: passage.excerpt,
            });
        }
    }
    Ok(())
}

pub async fn query(pool: &PgPool, q: &SearchQuery) -> Result<SearchPage> {
    if q.q.len() > 800
        || q.q.chars().count() > 200
        || q.q.split_whitespace().count() > 10
        || q.category.len() > 200
        || !["", "all", "notice", "document", "page"].contains(&q.kind.as_str())
        || !["", "current", "archive", "all"].contains(&q.state.as_str())
        || !["", "relevance", "newest", "oldest", "title"].contains(&q.sort.as_str())
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
        "id,kind,title,description,path,category,posted,archived",
    );
    b.push(" ORDER BY ");
    if !q.q.trim().is_empty() && matches!(q.sort.as_str(), "" | "relevance") {
        b.push("relevance ASC,");
    }
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
    let mut items: Vec<Hit> = b.build_query_as().fetch_all(&mut *tx).await?;
    passages(&mut tx, &mut items, q, day).await?;
    tx.commit().await?;
    for hit in &mut items {
        if hit.kind == "page" {
            hit.preview_html = Some(crate::markdown::render_search_preview(&hit.description));
        }
        // Preserve the original plain/raw API excerpt without cutting Markdown before parsing.
        if let Some((end, _)) = hit.description.char_indices().nth(400) {
            hit.description.truncate(end);
        }
    }
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
