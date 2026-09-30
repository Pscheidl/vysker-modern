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
        (SELECT count(*) FROM notices WHERE status='published') AS published,
        (SELECT count(*) FROM notices WHERE status='scheduled') AS scheduled,
        (SELECT count(*) FROM notices WHERE status='draft') AS drafts,
        (SELECT count(*) FROM documents WHERE status='published') AS documents,
        (SELECT count(*) FROM pages WHERE published=TRUE) AS pages,
        (SELECT count(*) FROM subscribers s WHERE verified_at IS NOT NULL AND unsubscribed_at IS NULL AND EXISTS(SELECT 1 FROM subscription_consents c WHERE c.subscriber_id=s.id AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL)) AS subscribers,
        (SELECT count(*) FROM mail_queue WHERE sent_at IS NULL AND cancelled=FALSE) AS pending_mail")
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
    Ok(Json(sqlx::query_as("SELECT * FROM notices WHERE ($1='' OR status=$2) AND title LIKE $3 ESCAPE '!' ORDER BY id DESC LIMIT $4 OFFSET $5")
        .bind(&f.status).bind(&f.status).bind(pattern).bind(limit).bind(offset).fetch_all(&s.pool).await?))
}
pub async fn documents(
    State(s): State<Backend>,
    _: Admin,
    Query(f): Query<Filter>,
) -> Result<Json<Vec<documents::Document>>> {
    let (limit, offset, pattern) = f.values(&["draft", "published", "archived"])?;
    Ok(Json(sqlx::query_as("SELECT * FROM documents WHERE ($1='' OR status=$2) AND title LIKE $3 ESCAPE '!' ORDER BY id DESC LIMIT $4 OFFSET $5")
        .bind(&f.status).bind(&f.status).bind(pattern).bind(limit).bind(offset).fetch_all(&s.pool).await?))
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
    }))
}
pub async fn document(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
) -> Result<Json<documents::DocumentDetail>> {
    let record = sqlx::query_as("SELECT * FROM documents WHERE id=$1")
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    let attachments = sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE document_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(&s.pool).await?;
    Ok(Json(documents::DocumentDetail {
        record,
        attachments,
    }))
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
        (EXISTS(SELECT 1 FROM notices WHERE id=attachments.notice_id AND status='draft') OR
         EXISTS(SELECT 1 FROM documents WHERE id=attachments.document_id AND status='draft'))",
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
    subscriber_id: i64,
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
    Ok(Json(sqlx::query_as("SELECT m.id,m.subscriber_id,s.email,m.purpose,m.subject,m.created_at,m.sent_at,m.attempts,CASE WHEN m.sent_at IS NOT NULL THEN 'sent' WHEN m.cancelled=TRUE THEN 'cancelled' WHEN m.attempts>0 THEN 'retrying' ELSE 'pending' END AS status FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id ORDER BY m.id DESC LIMIT $1 OFFSET $2")
        .bind(limit).bind(offset).fetch_all(&s.pool).await?))
}
