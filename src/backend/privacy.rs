use super::{Backend, Error, Result, audit, auth::Admin, timestamp};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// Administrators' saved policy overrides the server's initial configuration.
/// A read failure must never silently fall back to stale consent or retention rules.
pub async fn policy(s: &Backend) -> Result<Option<crate::privacy::PrivacyPolicy>> {
    let saved: Option<String> =
        sqlx::query_scalar("SELECT policy_json FROM privacy_settings WHERE id=1")
            .fetch_optional(&s.pool)
            .await?;
    decode_policy(saved, s)
}

pub async fn policy_in(
    s: &Backend,
    conn: &mut sqlx::PgConnection,
) -> Result<Option<crate::privacy::PrivacyPolicy>> {
    let saved: Option<String> =
        sqlx::query_scalar("SELECT policy_json FROM privacy_settings WHERE id=1")
            .fetch_optional(conn)
            .await?;
    decode_policy(saved, s)
}

pub async fn notice_in(
    s: &Backend,
    conn: &mut sqlx::PgConnection,
) -> Result<crate::privacy::PrivacyNotice> {
    policy_in(s, conn).await?.map(|p| p.notice()).ok_or(Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Odběr novinek zatím není aktivní.",
    ))
}

fn decode_policy(
    saved: Option<String>,
    s: &Backend,
) -> Result<Option<crate::privacy::PrivacyPolicy>> {
    match saved {
        Some(json) => serde_json::from_str(&json).map(Some).map_err(|_| {
            Error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Uložené informace o soukromí se nepodařilo načíst.",
            )
        }),
        None => Ok(s.config.privacy.clone()),
    }
}

pub async fn notice(s: &Backend) -> Result<crate::privacy::PrivacyNotice> {
    policy(s).await?.map(|p| p.notice()).ok_or(Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Odběr novinek zatím není aktivní.",
    ))
}

pub async fn public_notice(
    State(s): State<Backend>,
) -> Result<Json<Option<crate::privacy::PrivacyNotice>>> {
    Ok(Json(policy(&s).await?.map(|p| p.notice())))
}

pub async fn admin_notice(
    State(s): State<Backend>,
    _: Admin,
) -> Result<Json<Option<crate::privacy::PrivacyNotice>>> {
    public_notice(State(s)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyUpdate {
    policy: crate::privacy::PrivacyPolicy,
    fingerprint: Option<String>,
}

pub fn validate_policy(
    policy: &crate::privacy::PrivacyPolicy,
    production: bool,
) -> anyhow::Result<()> {
    policy.validate()?;
    if production {
        anyhow::ensure!(
            policy.approved_on.is_some(),
            "Produkce vyžaduje schválené informace o soukromí"
        );
        let json = serde_json::to_string(policy)?;
        anyhow::ensure!(
            !json.contains("DOPLNIT") && !json.contains("example.test"),
            "Nahraďte ukázkové údaje v nastavení soukromí"
        );
    }
    Ok(())
}

pub async fn update(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<PolicyUpdate>,
) -> Result<Json<crate::privacy::PrivacyNotice>> {
    validate_policy(&input.policy, s.config.production).map_err(|_| super::bad(
        "Zkontrolujte povinné texty, e-maily, datum schválení a lhůty uchování. V produkci vyplňte schválené údaje bez ukázkových hodnot.",
    ))?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let saved: Option<String> =
        sqlx::query_scalar("SELECT policy_json FROM privacy_settings WHERE id=1")
            .fetch_optional(&mut *tx)
            .await?;
    let current = decode_policy(saved, &s)?.map(|p| p.notice().fingerprint);
    if current != input.fingerprint {
        return Err(super::conflict(
            "Informace mezitím změnil jiný správce. Obnovte stránku a zopakujte úpravu.",
        ));
    }
    let notice = input.policy.notice();
    sqlx::query("INSERT INTO privacy_settings(id,policy_json,updated_at,updated_by) VALUES (1,$1,$2,$3) ON CONFLICT(id) DO UPDATE SET policy_json=excluded.policy_json,updated_at=excluded.updated_at,updated_by=excluded.updated_by")
        .bind(serde_json::to_string(&input.policy).expect("serializable policy"))
        .bind(OffsetDateTime::now_utc().unix_timestamp()).bind(admin.id)
        .execute(&mut *tx).await?;
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "privacy",
        1,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(notice))
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
    super::report_abuse::maintenance(&s.pool, now.unix_timestamp()).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let Some(policy) = policy_in(s, &mut tx).await? else {
        return Ok(());
    };
    let r = &policy.retention;
    let cutoff = |days: u16| now.unix_timestamp() - i64::from(days) * 86400;
    // Keep document delivery pending through outages. Retention removes completed
    // history and verification mail, never records owned by an SMTP worker.
    let mail = sqlx::query("DELETE FROM mail_queue WHERE created_at<$1 AND locked_until<=$2 AND (purpose<>'document' OR sent_at IS NOT NULL OR cancelled=TRUE)")
        .bind(cutoff(r.mail_days))
        .bind(now.unix_timestamp())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let subscribers = sqlx::query("DELETE FROM subscribers WHERE ((unsubscribed_at IS NOT NULL AND unsubscribed_at<$1) OR (unsubscribed_at IS NULL AND retention_started_at<$2 AND NOT EXISTS(SELECT 1 FROM subscription_consents c WHERE c.subscriber_id=subscribers.id AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL))) AND NOT EXISTS(SELECT 1 FROM mail_queue m WHERE m.subscriber_id=subscribers.id AND m.locked_until>$3)")
        .bind(cutoff(r.withdrawn_days)).bind(cutoff(r.pending_days)).bind(now.unix_timestamp()).execute(&mut *tx).await?.rows_affected();
    let consents = sqlx::query("DELETE FROM subscription_consents WHERE (withdrawn_at<$1 OR (confirmed_at IS NULL AND requested_at<$2 AND NOT EXISTS(SELECT 1 FROM subscription_tokens t WHERE t.consent_id=subscription_consents.id AND t.purpose='verification' AND t.expires_at>$3))) AND NOT EXISTS(SELECT 1 FROM mail_queue m WHERE m.consent_id=subscription_consents.id AND m.locked_until>$3)")
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
    let frontend_asset = request.uri().path().starts_with("/pkg/")
        || request.uri().path().starts_with("/relief/")
        || matches!(
            request.uri().path(),
            "/theme.js" | "/subscription-page.js" | "/relief-loader.js"
        );
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    let start = std::time::Instant::now();
    let mut response = next.run(request).await;
    tracing::debug!(%method, %route, status=response.status().as_u16(), elapsed_ms=start.elapsed().as_millis(), "HTTP");
    let headers = response.headers_mut();
    let html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if protect_documents || html {
        headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    } else if frontend_asset {
        // Revalidate stable asset URLs instead of using Last-Modified heuristics.
        headers.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    }
    if protect_documents {
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
        "camera=(), microphone=(), geolocation=(self)"
            .parse()
            .unwrap(),
    );
    if config.production {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000".parse().unwrap(),
        );
    }
    response
}
