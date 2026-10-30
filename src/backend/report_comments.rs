//! Signed public replies written and managed by authenticated administrators.
use super::{Backend, Result, audit, auth, bad, conflict, missing, reports, timestamp};
use crate::reports::{REPORT_COMMENT_LIMIT, REPORT_NAME_LIMIT, REPORT_UPLOAD_BYTES, ReportComment};
use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;
use sqlx::PgPool;
use std::collections::BTreeSet;
use time::OffsetDateTime;
use time_tz::OffsetDateTimeExt;

const COMMENT_BY_ID: &str = "SELECT id,report_id,author_name,body,created_at,updated_at,ARRAY(SELECT id FROM issue_report_comment_photos WHERE comment_id=c.id ORDER BY id) AS photo_ids FROM issue_report_comments c WHERE report_id=$1 AND id=$2";

async fn require_current_session(conn: &mut sqlx::PgConnection, admin: &auth::Admin) -> Result<()> {
    // Upload decoding may take time. Revocation, expiry or account deactivation
    // during that work must still prevent the final write.
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions s JOIN administrators a ON a.id=s.administrator_id WHERE s.token_hash=$1 AND s.administrator_id=$2 AND s.expires_at>$3 AND a.active=TRUE)")
        .bind(&admin.token_hash).bind(admin.id).bind(OffsetDateTime::now_utc().unix_timestamp())
        .fetch_one(conn).await?;
    if !active {
        return Err(auth::denied());
    }
    Ok(())
}

async fn comments_from_pool(pool: &PgPool, id: i64, admin: bool) -> Result<Vec<ReportComment>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM issue_reports WHERE id=$1 AND (hidden=FALSE OR $2))",
    )
    .bind(id)
    .bind(admin)
    .fetch_one(pool)
    .await?;
    if !exists {
        return Err(missing());
    }
    // Repeat the visibility condition in the read itself, including when moderation
    // changes a report between the existence check and this query.
    Ok(sqlx::query_as("SELECT c.id,c.report_id,c.author_name,c.body,c.created_at,c.updated_at,ARRAY(SELECT id FROM issue_report_comment_photos WHERE comment_id=c.id ORDER BY id) AS photo_ids FROM issue_report_comments c JOIN issue_reports r ON r.id=c.report_id WHERE r.id=$1 AND (r.hidden=FALSE OR $2) ORDER BY c.id")
        .bind(id).bind(admin).fetch_all(pool).await?)
}

pub async fn public_comments_from_pool(pool: &PgPool, id: i64) -> Result<Vec<ReportComment>> {
    comments_from_pool(pool, id, false).await
}

pub async fn public_list(
    State(s): State<Backend>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ReportComment>>> {
    Ok(Json(public_comments_from_pool(&s.pool, id).await?))
}

pub async fn admin_list(
    State(s): State<Backend>,
    _: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ReportComment>>> {
    Ok(Json(comments_from_pool(&s.pool, id, true).await?))
}

struct Submission {
    author_name: String,
    body: String,
    expected_updated_at: Option<String>,
    remove_photo_ids: BTreeSet<i64>,
    photos: Vec<Vec<u8>>,
    processed_bytes: usize,
}

async fn read_submission(multipart: &mut Multipart, editing: bool) -> Result<Submission> {
    let mut seen = BTreeSet::new();
    let (mut author_name, mut body, mut expected_updated_at) = (None, None, None);
    let mut remove_photo_ids = BTreeSet::new();
    let mut photos = Vec::new();
    let (mut upload_bytes, mut processed_bytes) = (0, 0);
    while let Some(mut field) =
        tokio::time::timeout(std::time::Duration::from_secs(60), multipart.next_field())
            .await
            .map_err(|_| bad("Přenos dat se přerušil. Zkuste odeslání znovu."))?
            .map_err(|_| bad("Komentář nelze přečíst nebo překračuje povolenou velikost."))?
    {
        let key = field.name().unwrap_or("").to_owned();
        if key == "photo" {
            if let Some(photo) =
                reports::read_photo(&mut field, &mut upload_bytes, &mut processed_bytes).await?
            {
                photos.push(photo);
            }
            continue;
        }
        if !seen.insert(key.clone()) {
            return Err(bad("Každé textové pole může být v komentáři jen jednou."));
        }
        match key.as_str() {
            "author_name" => {
                author_name = reports::optional_text(
                    reports::read_text_field(&mut field, REPORT_NAME_LIMIT).await?,
                    REPORT_NAME_LIMIT,
                    false,
                )?;
            }
            "body" => {
                body = reports::optional_text(
                    reports::read_text_field(&mut field, REPORT_COMMENT_LIMIT).await?,
                    REPORT_COMMENT_LIMIT,
                    true,
                )?;
            }
            "expected_updated_at" if editing => {
                expected_updated_at = reports::optional_text(
                    reports::read_text_field(&mut field, 64).await?,
                    64,
                    false,
                )?;
            }
            "remove_photo_ids" if editing => {
                let value = reports::read_text_field(&mut field, 8_192).await?;
                let ids: Vec<i64> = serde_json::from_str(&value)
                    .map_err(|_| bad("Neplatný seznam fotografií k odebrání."))?;
                for id in ids {
                    if id <= 0 || !remove_photo_ids.insert(id) {
                        return Err(bad("Neplatný seznam fotografií k odebrání."));
                    }
                }
            }
            _ => return Err(bad("Komentář obsahuje neznámé pole.")),
        }
    }
    if editing && expected_updated_at.is_none() {
        return Err(bad("Chybí verze komentáře. Obnovte seznam komentářů."));
    }
    Ok(Submission {
        author_name: author_name.ok_or_else(|| bad("Vyplňte jméno autora komentáře."))?,
        body: body.ok_or_else(|| bad("Napište text komentáře."))?,
        expected_updated_at,
        remove_photo_ids,
        photos,
        processed_bytes,
    })
}

async fn submission(multipart: &mut Multipart, editing: bool) -> Result<Submission> {
    tokio::time::timeout(
        std::time::Duration::from_secs(15 * 60),
        read_submission(multipart, editing),
    )
    .await
    .map_err(|_| bad("Odesílání trvalo příliš dlouho. Zkuste ho prosím znovu."))?
}

pub async fn create(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<ReportComment>)> {
    let input = submission(&mut multipart, false).await?;
    let now = OffsetDateTime::now_utc();
    let at = timestamp(now.to_timezone(time_tz::timezones::db::europe::PRAGUE));
    let mut tx = crate::db::begin_write(&s.pool).await?;
    require_current_session(&mut tx, &admin).await?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM issue_reports WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    let comment_id: i64 = sqlx::query_scalar("INSERT INTO issue_report_comments(report_id,administrator_id,author_name,body,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$5) RETURNING id")
        .bind(id).bind(admin.id).bind(input.author_name).bind(input.body).bind(at)
        .fetch_one(&mut *tx).await?;
    for photo in input.photos {
        sqlx::query("INSERT INTO issue_report_comment_photos(comment_id,photo) VALUES ($1,$2)")
            .bind(comment_id)
            .bind(photo)
            .execute(&mut *tx)
            .await?;
    }
    audit(
        &mut tx,
        Some(admin.id),
        "created",
        "issue_report_comment",
        comment_id,
        now,
    )
    .await?;
    let comment = sqlx::query_as(COMMENT_BY_ID)
        .bind(id)
        .bind(comment_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(comment)))
}

pub async fn update(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path((id, comment_id)): Path<(i64, i64)>,
    mut multipart: Multipart,
) -> Result<Json<ReportComment>> {
    let input = submission(&mut multipart, true).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    require_current_session(&mut tx, &admin).await?;
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM issue_report_comments WHERE report_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(comment_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(missing)?;
    let current: ReportComment = sqlx::query_as(COMMENT_BY_ID)
        .bind(id)
        .bind(comment_id)
        .fetch_one(&mut *tx)
        .await?;
    if input.expected_updated_at.as_deref() != Some(&current.updated_at) {
        return Err(conflict(
            "Komentář mezitím upravil jiný administrátor. Obnovte seznam a zkontrolujte změny.",
        ));
    }
    let existing: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id,octet_length(photo)::bigint FROM issue_report_comment_photos WHERE comment_id=$1",
    )
    .bind(comment_id)
    .fetch_all(&mut *tx)
    .await?;
    if input
        .remove_photo_ids
        .iter()
        .any(|id| !existing.iter().any(|(existing_id, _)| existing_id == id))
    {
        return Err(bad(
            "Fotografie k odebrání nepatří k tomuto komentáři nebo už byla odstraněna.",
        ));
    }
    let retained_bytes: i64 = existing
        .iter()
        .filter(|(id, _)| !input.remove_photo_ids.contains(id))
        .map(|(_, bytes)| bytes)
        .sum();
    if retained_bytes > (REPORT_UPLOAD_BYTES - input.processed_bytes) as i64 {
        return Err(bad(
            "Fotografie komentáře mohou mít dohromady nejvýše 100 MB. Odeberte některé stávající fotografie.",
        ));
    }
    if input.author_name == current.author_name
        && input.body == current.body
        && input.remove_photo_ids.is_empty()
        && input.photos.is_empty()
    {
        tx.commit().await?;
        return Ok(Json(current));
    }
    let now = OffsetDateTime::now_utc();
    // Ensure a distinct optimistic-lock value even on a clock with low resolution.
    let mut at = now.to_timezone(time_tz::timezones::db::europe::PRAGUE);
    if timestamp(at) == current.updated_at {
        at += time::Duration::nanoseconds(1);
    }
    sqlx::query("UPDATE issue_report_comments SET author_name=$1,body=$2,updated_at=$3 WHERE report_id=$4 AND id=$5")
        .bind(input.author_name).bind(input.body).bind(timestamp(at)).bind(id).bind(comment_id)
        .execute(&mut *tx).await?;
    if !input.remove_photo_ids.is_empty() {
        sqlx::query("DELETE FROM issue_report_comment_photos WHERE comment_id=$1 AND id=ANY($2)")
            .bind(comment_id)
            .bind(input.remove_photo_ids.into_iter().collect::<Vec<_>>())
            .execute(&mut *tx)
            .await?;
    }
    for photo in input.photos {
        sqlx::query("INSERT INTO issue_report_comment_photos(comment_id,photo) VALUES ($1,$2)")
            .bind(comment_id)
            .bind(photo)
            .execute(&mut *tx)
            .await?;
    }
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "issue_report_comment",
        comment_id,
        now,
    )
    .await?;
    let comment = sqlx::query_as(COMMENT_BY_ID)
        .bind(id)
        .bind(comment_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(comment))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteComment {
    pub expected_updated_at: String,
}

pub async fn delete(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path((id, comment_id)): Path<(i64, i64)>,
    Json(input): Json<DeleteComment>,
) -> Result<StatusCode> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    require_current_session(&mut tx, &admin).await?;
    let at: String = sqlx::query_scalar(
        "SELECT updated_at FROM issue_report_comments WHERE report_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(comment_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(missing)?;
    if input.expected_updated_at != at {
        return Err(conflict(
            "Komentář mezitím upravil jiný administrátor. Obnovte seznam a zkontrolujte změny.",
        ));
    }
    sqlx::query("DELETE FROM issue_report_comments WHERE report_id=$1 AND id=$2")
        .bind(id)
        .bind(comment_id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        Some(admin.id),
        "deleted",
        "issue_report_comment",
        comment_id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn photo_response(
    s: &Backend,
    id: i64,
    comment_id: i64,
    photo_id: i64,
    admin: bool,
) -> Result<Response> {
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT p.photo FROM issue_report_comment_photos p JOIN issue_report_comments c ON c.id=p.comment_id JOIN issue_reports r ON r.id=c.report_id WHERE r.id=$1 AND c.id=$2 AND p.id=$3 AND (r.hidden=FALSE OR $4)")
        .bind(id).bind(comment_id).bind(photo_id).bind(admin).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    Ok(reports::image_response(bytes))
}

pub async fn photo(
    State(s): State<Backend>,
    Path((id, comment_id, photo_id)): Path<(i64, i64, i64)>,
) -> Result<Response> {
    photo_response(&s, id, comment_id, photo_id, false).await
}

pub async fn admin_photo(
    State(s): State<Backend>,
    _: auth::Admin,
    Path((id, comment_id, photo_id)): Path<(i64, i64, i64)>,
) -> Result<Response> {
    photo_response(&s, id, comment_id, photo_id, true).await
}
