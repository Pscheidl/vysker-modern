use super::{Backend, Error, Result, audit, auth::Admin, timestamp};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;
use time::{Duration, OffsetDateTime};

pub fn notice(s: &Backend) -> Result<crate::privacy::PrivacyNotice> {
    s.config.privacy.as_ref().map(|p| p.notice()).ok_or(Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Odběr novinek zatím není aktivní.",
    ))
}
pub async fn public_notice(
    State(s): State<Backend>,
) -> Json<Option<crate::privacy::PrivacyNotice>> {
    Json(s.config.privacy.as_ref().map(|p| p.notice()))
}

#[derive(sqlx::FromRow, Serialize)]
pub struct ConsentEvidence {
    id: i64,
    subscriber_id: i64,
    email: String,
    requested_at: i64,
    confirmed_at: Option<i64>,
    withdrawn_at: Option<i64>,
    superseded_at: Option<i64>,
    notice_fingerprint: String,
    version: String,
    consent_text: String,
    privacy_json: String,
}
pub async fn evidence(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ConsentEvidence>>> {
    Ok(Json(sqlx::query_as("SELECT c.id,c.subscriber_id,s.email,c.requested_at,c.confirmed_at,c.withdrawn_at,c.superseded_at,c.notice_fingerprint,n.version,n.consent_text,n.privacy_json FROM subscription_consents c JOIN subscribers s ON s.id=c.subscriber_id JOIN consent_notices n ON n.fingerprint=c.notice_fingerprint WHERE s.id=$1 ORDER BY c.id DESC LIMIT 1000")
        .bind(id).fetch_all(&s.pool).await?))
}

pub async fn maintenance(s: &Backend, now: OffsetDateTime) -> Result<()> {
    super::recovery::maintenance(s, now).await?;
    let Some(policy) = &s.config.privacy else {
        return Ok(());
    };
    let r = &policy.retention;
    let cutoff = |days: u16| now.unix_timestamp() - i64::from(days) * 86400;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    // Do not delete records while an SMTP worker owns their lease.
    let mail = sqlx::query("DELETE FROM mail_queue WHERE created_at<$1 AND locked_until<=$2")
        .bind(cutoff(r.mail_days))
        .bind(now.unix_timestamp())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let subscribers = sqlx::query("DELETE FROM subscribers WHERE ((unsubscribed_at IS NOT NULL AND unsubscribed_at<$1) OR (unsubscribed_at IS NULL AND retention_started_at<$2 AND NOT EXISTS(SELECT 1 FROM subscription_consents c WHERE c.subscriber_id=subscribers.id AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL))) AND NOT EXISTS(SELECT 1 FROM mail_queue m WHERE m.subscriber_id=subscribers.id AND m.locked_until>$3)")
        .bind(cutoff(r.withdrawn_days)).bind(cutoff(r.pending_days)).bind(now.unix_timestamp()).execute(&mut *tx).await?.rows_affected();
    let consents = sqlx::query("DELETE FROM subscription_consents WHERE (withdrawn_at<$1 OR (confirmed_at IS NULL AND requested_at<$2)) AND NOT EXISTS(SELECT 1 FROM mail_queue m WHERE m.consent_id=subscription_consents.id AND m.locked_until>$3)")
        .bind(cutoff(r.withdrawn_days)).bind(cutoff(r.pending_days)).bind(now.unix_timestamp()).execute(&mut *tx).await?.rows_affected();
    let audit_cutoff = timestamp(now - Duration::days(i64::from(r.audit_days)));
    sqlx::query("UPDATE privacy_maintenance SET audit_delete_before=$1 WHERE id=1")
        .bind(&audit_cutoff)
        .execute(&mut *tx)
        .await?;
    let events = sqlx::query("DELETE FROM audit_log WHERE occurred_at<$1")
        .bind(&audit_cutoff)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("UPDATE privacy_maintenance SET audit_delete_before=NULL WHERE id=1")
        .execute(&mut *tx)
        .await?;
    let notice_cutoff = timestamp(now - Duration::days(i64::from(r.notice_internal_days)));
    sqlx::query("UPDATE privacy_maintenance SET notice_delete_before=$1 WHERE id=1")
        .bind(&notice_cutoff)
        .execute(&mut *tx)
        .await?;
    let proofs=sqlx::query("DELETE FROM notice_events WHERE occurred_at<$1 AND notice_id IN (SELECT id FROM notices WHERE status IN ('archived','withdrawn') AND withdrawn_at<$2)")
        .bind(&notice_cutoff).bind(&notice_cutoff).execute(&mut *tx).await?.rows_affected();
    sqlx::query("UPDATE privacy_maintenance SET notice_delete_before=NULL WHERE id=1")
        .execute(&mut *tx)
        .await?;
    // Keep the permanent public record, remove internal text once its separate purpose expires.
    let records=sqlx::query("UPDATE notices SET title='Záznam úřední desky #'||id,description=NULL,reference_number=NULL,issuer=NULL,review_json='{}',retain_attachments=FALSE WHERE status IN ('archived','withdrawn') AND withdrawn_at<$1 AND (description IS NOT NULL OR reference_number IS NOT NULL OR issuer IS NOT NULL OR review_json<>'{}' OR title<>'Záznam úřední desky #'||id)")
        .bind(&notice_cutoff).execute(&mut *tx).await?.rows_affected();
    sqlx::query("UPDATE attachments SET name='Příloha #'||id,data=NULL,removed_at=COALESCE(removed_at,$1) WHERE notice_id IN (SELECT id FROM notices WHERE status IN ('archived','withdrawn') AND withdrawn_at<$2) AND (name<>'Příloha #'||id OR data IS NOT NULL OR removed_at IS NULL)")
        .bind(timestamp(now)).bind(&notice_cutoff).execute(&mut *tx).await?;
    if mail + subscribers + consents + events + proofs + records > 0 {
        audit(&mut tx, None, "retention_applied", "privacy", 1, now).await?;
        // Aggregate counts only. Never copy erased identities into another log.
        tracing::info!(
            mail,
            subscribers,
            consents,
            events,
            proofs,
            records,
            "proveden úklid osobních údajů"
        );
    }
    tx.commit().await?;
    Ok(())
}

/// No query strings, headers, addresses or request bodies are logged.
pub async fn response_headers(
    State(config): State<std::sync::Arc<crate::config::Config>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::header;
    let method = request.method().clone();
    let protect_documents = request.uri().path().starts_with("/uredni-deska")
        || request.uri().path().starts_with("/api/")
        || request.uri().path() == "/"
        || request.uri().path() == "/hledat";
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    let start = std::time::Instant::now();
    let mut response = next.run(request).await;
    tracing::debug!(%method, %route, status=response.status().as_u16(), elapsed_ms=start.elapsed().as_millis(), "HTTP");
    let headers = response.headers_mut();
    if protect_documents {
        headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        headers.insert("x-robots-tag", "noindex, noarchive".parse().unwrap());
    }
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    headers.entry(header::CONTENT_SECURITY_POLICY).or_insert(
        "frame-ancestors 'none'; base-uri 'self'; object-src 'none'"
            .parse()
            .unwrap(),
    );
    headers.insert(
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()".parse().unwrap(),
    );
    if config.production {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000".parse().unwrap(),
        );
    }
    response
}
