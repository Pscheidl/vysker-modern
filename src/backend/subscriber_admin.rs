//! Authorized subscriber evidence access, consent withdrawal and identity erasure.
//! Public double opt-in remains the only way to activate a subscription.
use super::{Backend, Result, accounts::Credentials, audit, auth, bad, missing, timestamp};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

// A legacy verified address without current consent must never appear as active.
const SUBSCRIBERS: &str = "WITH classified AS (
    SELECT s.id,s.email,s.verified_at,s.unsubscribed_at,s.retention_started_at,
        CASE WHEN s.unsubscribed_at IS NOT NULL THEN 'unsubscribed'
             WHEN s.verified_at IS NOT NULL AND EXISTS (
                 SELECT 1 FROM subscription_consents c WHERE c.subscriber_id=s.id
                 AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL
                 AND c.superseded_at IS NULL
             ) THEN 'active' ELSE 'pending' END AS status
    FROM subscribers s
)";
const FILTER: &str = " WHERE (strpos(email,$1)>0 OR strpos(email,$2)>0)
    AND ($3='' OR status=$3)";

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    pub q: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Subscriber {
    id: i64,
    email: String,
    status: String,
    verified_at: Option<i64>,
    unsubscribed_at: Option<i64>,
    retention_started_at: i64,
}

#[derive(Serialize)]
pub struct SubscriberList {
    items: Vec<Subscriber>,
    total: i64,
    limit: i64,
    offset: i64,
}

pub async fn list(
    State(s): State<Backend>,
    _: auth::Admin,
    Query(input): Query<ListQuery>,
) -> Result<Json<SubscriberList>> {
    let q = input.q.unwrap_or_default();
    if q.len() > 254 || q.chars().any(char::is_control) {
        return Err(bad(
            "Hledaný e-mail je příliš dlouhý nebo obsahuje nepovolené znaky.",
        ));
    }
    let q = q.trim();
    let normalized = q.to_lowercase();
    let status = input.status.unwrap_or_default();
    if !matches!(status.as_str(), "" | "active" | "pending" | "unsubscribed") {
        return Err(bad("Neplatný stav odběru."));
    }
    let limit = input.limit.unwrap_or(20);
    let offset = input.offset.unwrap_or(0);
    if !(1..=100).contains(&limit) || !(0..=i64::MAX - 100).contains(&offset) {
        return Err(bad(
            "Neplatné stránkování. Velikost stránky musí být 1 až 100.",
        ));
    }
    // Count and page share a snapshot, including an empty page beyond the last row.
    let mut tx = s.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    // strpos treats percent, underscore, backslash and quotes as literal characters.
    let total = sqlx::QueryBuilder::<sqlx::Postgres>::new(SUBSCRIBERS)
        .push(" SELECT count(*) FROM classified")
        .push(FILTER)
        .build_query_scalar()
        .bind(q)
        .bind(&normalized)
        .bind(&status)
        .fetch_one(&mut *tx)
        .await?;
    let items = sqlx::QueryBuilder::<sqlx::Postgres>::new(SUBSCRIBERS)
        .push(" SELECT * FROM classified")
        .push(FILTER)
        .push(" ORDER BY email,id LIMIT $4 OFFSET $5")
        .build_query_as()
        .bind(q)
        .bind(&normalized)
        .bind(&status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(SubscriberList {
        items,
        total,
        limit,
        offset,
    }))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExportQuery {
    pub download: Option<bool>,
}

#[derive(Serialize, sqlx::FromRow)]
struct ConsentEvidence {
    id: i64,
    requested_at: i64,
    confirmed_at: Option<i64>,
    withdrawn_at: Option<i64>,
    superseded_at: Option<i64>,
    notice_fingerprint: String,
    version: String,
    consent_text: String,
    // Preserve the exact stored snapshot, including old policy fields and formatting.
    privacy_json: String,
}

#[derive(Serialize, sqlx::FromRow)]
struct MailEvidence {
    id: i64,
    consent_id: Option<i64>,
    purpose: String,
    attempts: i64,
    created_at: i64,
    next_attempt_at: i64,
    locked_until: i64,
    sent_at: Option<i64>,
    cancelled: bool,
}

#[derive(Serialize, sqlx::FromRow)]
struct AuditEvidence {
    id: i64,
    occurred_at: String,
    operation: String,
}

#[derive(Serialize)]
struct SubscriberExport {
    exported_at: String,
    subscriber: Subscriber,
    consents: Vec<ConsentEvidence>,
    mail_history: Vec<MailEvidence>,
    audit_history: Vec<AuditEvidence>,
    mail_history_note: &'static str,
}

pub async fn export(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Query(input): Query<ExportQuery>,
) -> Result<Response> {
    let mut tx = s.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    let subscriber = sqlx::QueryBuilder::<sqlx::Postgres>::new(SUBSCRIBERS)
        .push(" SELECT * FROM classified WHERE id=$1")
        .build_query_as()
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    let consents = sqlx::query_as("SELECT c.id,c.requested_at,c.confirmed_at,c.withdrawn_at,c.superseded_at,c.notice_fingerprint,n.version,n.consent_text,n.privacy_json FROM subscription_consents c JOIN consent_notices n ON n.fingerprint=c.notice_fingerprint WHERE c.subscriber_id=$1 ORDER BY c.id")
        .bind(id).fetch_all(&mut *tx).await?;
    // Explicit allowlist excludes message bodies, tokens, lock secrets and their hashes.
    let mail_history = sqlx::query_as("SELECT id,consent_id,purpose,attempts,created_at,next_attempt_at,locked_until,sent_at,cancelled FROM mail_queue WHERE subscriber_id=$1 ORDER BY id")
        .bind(id).fetch_all(&mut *tx).await?;
    let now = OffsetDateTime::now_utc();
    audit(
        &mut tx,
        Some(admin.id),
        "subscriber_exported",
        "subscriber",
        id,
        now,
    )
    .await?;
    let audit_history = sqlx::query_as("SELECT id,occurred_at,operation FROM audit_log WHERE entity_type='subscriber' AND entity_id=$1 ORDER BY id")
        .bind(id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let mut response = Json(SubscriberExport {
        exported_at: timestamp(now), subscriber, consents, mail_history, audit_history,
        mail_history_note: "Historie obsahuje dosud uchované zprávy, počet pokusů a stav předání SMTP. Potvrzení SMTP nedokládá přečtení ani doručení do schránky. Jednotlivé pokusy se samostatně neuchovávají. Číselné časy jsou Unix sekundy v UTC.",
    }).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    if input.download.unwrap_or(false) {
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"odberatel-{id}.json\"")
                .parse()
                .unwrap(),
        );
    }
    Ok(response)
}

// Match account reauthentication without holding a write lock during Argon2.
async fn reauthenticate(s: &Backend, admin: &auth::Admin, password: String) -> Result<String> {
    auth::throttle(
        s,
        &format!("reauth:{}", admin.id),
        10,
        900,
        OffsetDateTime::now_utc(),
    )
    .await?;
    let stored: String =
        sqlx::query_scalar("SELECT password_hash FROM administrators WHERE id=$1 AND active=TRUE")
            .bind(admin.id)
            .fetch_optional(&s.pool)
            .await?
            .ok_or_else(auth::denied)?;
    auth::verify_password(password, stored.clone()).await?;
    Ok(stored)
}

// A password change, deactivation or session revocation may race the password check.
async fn current(conn: &mut sqlx::PgConnection, admin: &auth::Admin, verified: &str) -> Result<()> {
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators a JOIN sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND a.active=TRUE AND a.password_hash=$2 AND s.token_hash=$3 AND s.expires_at>$4)")
        .bind(admin.id).bind(verified).bind(&admin.token_hash)
        .bind(OffsetDateTime::now_utc().unix_timestamp()).fetch_one(conn).await?;
    if valid { Ok(()) } else { Err(auth::denied()) }
}

pub async fn withdraw(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<Credentials>,
) -> Result<StatusCode> {
    let verified = reauthenticate(&s, &admin, input.current_password).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    current(&mut tx, &admin, &verified).await?;
    let withdrawn: Option<i64> =
        sqlx::query_scalar("SELECT unsubscribed_at FROM subscribers WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(missing)?;
    let now = OffsetDateTime::now_utc();
    sqlx::query("UPDATE subscribers SET unsubscribed_at=COALESCE(unsubscribed_at,$1) WHERE id=$2")
        .bind(now.unix_timestamp())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE subscription_consents SET withdrawn_at=$1 WHERE subscriber_id=$2 AND withdrawn_at IS NULL AND superseded_at IS NULL")
        .bind(now.unix_timestamp()).bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM subscription_tokens WHERE subscriber_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // Keep an in-flight lease intact until the worker returns, so erasure still waits.
    sqlx::query(
        "UPDATE mail_queue SET cancelled=TRUE,body=NULL WHERE subscriber_id=$1 AND sent_at IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if withdrawn.is_none() {
        audit(
            &mut tx,
            Some(admin.id),
            "subscriber_withdrawn",
            "subscriber",
            id,
            now,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn erase(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<Credentials>,
) -> Result<Response> {
    let verified = reauthenticate(&s, &admin, input.current_password).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    current(&mut tx, &admin, &verified).await?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM subscribers WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    // SMTP claims rows outside the business advisory lock. Row locks close the race
    // between checking leases and the cascading delete, including cancelled rows.
    let leases: Vec<i64> = sqlx::query_scalar(
        "SELECT locked_until FROM mail_queue WHERE subscriber_id=$1 ORDER BY id FOR UPDATE",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let now = OffsetDateTime::now_utc();
    if let Some(until) = leases
        .into_iter()
        .max()
        .filter(|until| *until > now.unix_timestamp())
    {
        let retry = until.saturating_sub(now.unix_timestamp());
        tx.rollback().await?;
        let mut response = (StatusCode::CONFLICT, Json(serde_json::json!({
            "error": format!("Právě probíhá odesílání zprávy. Výmaz zatím nelze provést. Zkuste jej znovu za {retry} sekund. Odběr můžete ihned odvolat."),
            "retry_after_seconds": retry,
        }))).into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, retry.to_string().parse().unwrap());
        return Ok(response);
    }
    // Foreign keys remove consents, tokens and all retained mail with the identity.
    // Shared immutable notice snapshots do not identify the erased subscriber.
    sqlx::query("DELETE FROM subscribers WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        Some(admin.id),
        "subscriber_erased",
        "subscriber",
        id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
