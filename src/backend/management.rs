//! Čtecí rozhraní administrace a odebrání přílohy konceptu.
use super::{
    Backend, Pagination, Result, audit, auth::Admin, bad, conflict, documents, missing, notices,
    pages, timestamp, today,
};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Deserialize, Default)]
pub struct Filter {
    // serde_urlencoded musí číselné položky číst přímo, ne přes flatten.
    limit: Option<i64>,
    offset: Option<i64>,
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    state: String,
}
impl Filter {
    fn values(&self, states: &[&str]) -> Result<(i64, i64, String)> {
        if self.q.len() > 500
            || (!self.status.is_empty() && !states.contains(&self.status.as_str()))
        {
            return Err(bad("Neplatný filtr."));
        }
        let (limit, offset) = Pagination {
            limit: self.limit,
            offset: self.offset,
        }
        .bounds()?;
        let pattern = format!(
            "%{}%",
            self.q
                .trim()
                .replace('!', "!!")
                .replace('%', "!%")
                .replace('_', "!_")
        );
        Ok((limit, offset, pattern))
    }
}

#[derive(sqlx::FromRow, Serialize)]
pub struct Overview {
    published: i64,
    scheduled: i64,
    drafts: i64,
    documents: i64,
    pages: i64,
    subscribers: i64,
    pending_mail: i64,
    #[sqlx(default)]
    current_date: String,
}
pub async fn overview(State(s): State<Backend>, _: Admin) -> Result<Json<Overview>> {
    let mut row: Overview = sqlx::query_as("SELECT
        (SELECT count(*) FROM documents WHERE has_current_posting) AS published,
        (SELECT count(*) FROM documents WHERE board_status='scheduled') AS scheduled,
        (SELECT count(*) FROM documents WHERE status='draft' AND board_status IS DISTINCT FROM 'scheduled') AS drafts,
        (SELECT count(*) FROM documents WHERE status='published') AS documents,
        (SELECT count(*) FROM pages WHERE published=TRUE) AS pages,
        (SELECT count(*) FROM subscribers s WHERE verified_at IS NOT NULL AND unsubscribed_at IS NULL AND EXISTS(SELECT 1 FROM subscription_consents c WHERE c.subscriber_id=s.id AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL)) AS subscribers,
        (SELECT count(*) FROM mail_queue WHERE sent_at IS NULL AND cancelled=FALSE)
            + (SELECT count(*) FROM publication_outbox) AS pending_mail")
        .fetch_one(&s.pool).await?;
    row.current_date = today(OffsetDateTime::now_utc()).to_string();
    Ok(Json(row))
}
pub async fn notices(
    State(s): State<Backend>,
    _: Admin,
    Query(f): Query<Filter>,
) -> Result<Json<Vec<notices::NoticeRecord>>> {
    let (limit, offset, pattern) =
        f.values(&["draft", "scheduled", "published", "archived", "withdrawn"])?;
    Ok(Json(sqlx::query_as("SELECT * FROM notices WHERE ($1='' OR status=$2) AND title LIKE $3 ESCAPE '!' ORDER BY published_on DESC NULLS LAST,id DESC LIMIT $4 OFFSET $5")
        .bind(&f.status).bind(&f.status).bind(pattern).bind(limit).bind(offset).fetch_all(&s.pool).await?))
}
pub async fn documents(
    State(s): State<Backend>,
    _: Admin,
    Query(f): Query<Filter>,
) -> Result<Json<Vec<documents::Document>>> {
    let (limit, offset, pattern) = f.values(&["draft", "published", "archived"])?;
    if !["", "all", "current", "scheduled", "archive", "draft"].contains(&f.state.as_str()) {
        return Err(bad("Neplatný filtr dokumentů."));
    }
    let rows:Vec<documents::Document>=sqlx::query_as("SELECT id,title,left(description,400) AS description,status,created_at,published_at,source_published_on,document_date,category_id,reference_number,issuer,lock_state,public_access,access_blocked,public_until,board_status,published_on,withdraw_on,retain_attachments,posting_after_action,content_version,metadata_restricted,public_title,attachments_public,posting_public_until FROM documents WHERE ($1='' OR status=$1) AND title ILIKE $2 ESCAPE '!' AND CASE $5 WHEN 'current' THEN has_current_posting WHEN 'scheduled' THEN board_status='scheduled' WHEN 'archive' THEN ((board_status IS NULL AND status='archived') OR has_removed_posting) AND NOT has_current_posting AND board_status IS DISTINCT FROM 'scheduled' WHEN 'draft' THEN status='draft' AND board_status IS DISTINCT FROM 'scheduled' ELSE TRUE END ORDER BY catalog_date DESC NULLS LAST,id DESC LIMIT $3 OFFSET $4")
        .bind(&f.status).bind(pattern).bind(limit).bind(offset).bind(&f.state).fetch_all(&s.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(documents::Document::with_board_settings)
            .collect(),
    ))
}
pub async fn pages(
    State(s): State<Backend>,
    _: Admin,
    Query(f): Query<Filter>,
) -> Result<Json<Vec<pages::Page>>> {
    let (limit, offset, pattern) = f.values(&["draft", "published"])?;
    Ok(Json(sqlx::query_as("SELECT * FROM pages WHERE ($1='' OR published=$2) AND title LIKE $3 ESCAPE '!' ORDER BY id DESC LIMIT $4 OFFSET $5")
        .bind(&f.status).bind(f.status=="published").bind(pattern).bind(limit).bind(offset).fetch_all(&s.pool).await?))
}
pub async fn notice(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
) -> Result<Json<notices::NoticeDetail>> {
    let record = sqlx::query_as("SELECT * FROM notices WHERE id=$1")
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    let attachments = documents::notice_files(&s, id).await?;
    Ok(Json(notices::NoticeDetail {
        record,
        attachments,
        postings: super::registry::postings(&s.pool, id, false).await?,
        relations: documents::public_relations(&s.pool, id).await?,
        origin: super::registry::origin(&s.pool, id).await?,
    }))
}
pub async fn document(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
) -> Result<Json<documents::DocumentDetail>> {
    let record: documents::Document = sqlx::query_as("SELECT * FROM documents WHERE id=$1")
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    let attachments = sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE document_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(&s.pool).await?;
    Ok(Json(documents::DocumentDetail {
        record: record.with_board_settings(),
        attachments,
        postings: super::registry::postings(&s.pool, id, false).await?,
        relations: documents::public_relations(&s.pool, id).await?,
        origin: super::registry::origin(&s.pool, id).await?,
    }))
}
/// Resolve old notice bookmarks before looking at the new shared ID namespace.
pub async fn legacy_notice(
    State(s): State<Backend>,
    _: Admin,
    Path(old_id): Path<i64>,
) -> Result<Json<serde_json::Value>> {
    let id: Option<i64> = sqlx::query_scalar("SELECT coalesce((SELECT document_id FROM document_legacy_ids WHERE old_model='notice' AND old_id=$1),(SELECT id FROM documents WHERE id=$1 AND board_status IS NOT NULL))")
        .bind(old_id).fetch_one(&s.pool).await?;
    Ok(Json(serde_json::json!({"id":id.ok_or_else(missing)?})))
}

pub async fn page(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
) -> Result<Json<pages::Page>> {
    Ok(Json(
        sqlx::query_as("SELECT * FROM pages WHERE id=$1")
            .bind(id)
            .fetch_optional(&s.pool)
            .await?
            .ok_or_else(missing)?,
    ))
}
pub async fn download(State(s): State<Backend>, _: Admin, Path(id): Path<i64>) -> Result<Response> {
    let (name, mime, bytes): (String, String, Vec<u8>) = sqlx::query_as("SELECT name,content_type,data FROM attachments WHERE id=$1 AND data IS NOT NULL AND removed_at IS NULL")
        .bind(id).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    let encoded = percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC);
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"dokument\"; filename*=UTF-8''{encoded}"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'".into(),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}
pub async fn remove_file(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let now = OffsetDateTime::now_utc();
    let changed = sqlx::query(
        "UPDATE attachments SET data=NULL,removed_at=$1 WHERE id=$2 AND removed_at IS NULL AND
        EXISTS(SELECT 1 FROM documents WHERE id=attachments.document_id AND status='draft' AND lock_state='draft' AND board_status IS DISTINCT FROM 'scheduled')",
    )
    .bind(timestamp(now))
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(conflict("Odebrat lze pouze přílohu konceptu."));
    }
    audit(&mut tx, Some(admin.id), "removed", "attachment", id, now).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow, Serialize)]
pub struct MailRecord {
    id: i64,
    subscriber_id: Option<i64>,
    email: String,
    purpose: String,
    subject: String,
    created_at: i64,
    sent_at: Option<i64>,
    attempts: i64,
    status: String,
}
pub async fn mail_history(
    State(s): State<Backend>,
    _: Admin,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<MailRecord>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(sqlx::query_as("SELECT id,subscriber_id,email,purpose,subject,created_at,sent_at,attempts,CASE WHEN sent_at IS NOT NULL THEN 'sent' WHEN delivery_expired_at IS NOT NULL THEN 'expired' WHEN cancelled=TRUE THEN 'cancelled' WHEN attempts>0 THEN 'retrying' ELSE 'pending' END AS status FROM (SELECT m.id,m.subscriber_id,s.email,m.purpose,m.subject,m.created_at,m.sent_at,m.attempts,m.cancelled,NULL::BIGINT AS delivery_expired_at FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id UNION ALL SELECT m.id,NULL::BIGINT,a.email,'recovery','Obnova hesla správy webu Vyskeř',m.created_at,m.sent_at,m.attempts,m.cancelled,m.delivery_expired_at FROM recovery_mail m JOIN administrators a ON a.id=m.administrator_id) messages ORDER BY created_at DESC,purpose,id DESC LIMIT $1 OFFSET $2")
        .bind(limit).bind(offset).fetch_all(&s.pool).await?))
}
