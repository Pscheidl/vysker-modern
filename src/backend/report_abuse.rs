//! First-party submission proofs and restart-safe abuse limits.
//! A browser token or coarse fingerprint indicates continuity, never a person's identity.
use super::{Backend, Error, Result, auth, bad};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use ring::hmac;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;

const COOKIE: &str = "obec_report_browser";
const COOKIE_SECONDS: i64 = 30 * 86_400;
const TOKEN_SECONDS: i64 = 2 * 60 * 60;
const RETRY_SECONDS: i64 = 86_400;
pub static SUBMISSION_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Debug)]
pub enum SubmissionError {
    Standard(Error),
    InvalidToken,
}
impl From<Error> for SubmissionError {
    fn from(value: Error) -> Self {
        Self::Standard(value)
    }
}
impl From<sqlx::Error> for SubmissionError {
    fn from(value: sqlx::Error) -> Self {
        Self::Standard(value.into())
    }
}
impl IntoResponse for SubmissionError {
    fn into_response(self) -> Response {
        match self {
            Self::Standard(error) => error.into_response(),
            Self::InvalidToken => (StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "Platnost formuláře vypršela. Obnovte ověření a odešlete hlášení znovu.",
                "code": "report_token_invalid"
            }))).into_response(),
        }
    }
}
pub type SubmissionResult<T> = std::result::Result<T, SubmissionError>;

pub fn check_origin(headers: &HeaderMap, state: &Backend) -> Result<()> {
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(state.config.public_url.as_str())
    {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Požadavek musí pocházet z tohoto webu.",
        ));
    }
    Ok(())
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    // Duplicate cookies are ambiguous, so do not choose an attacker-controlled one.
    let mut values = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|part| part.trim().split_once('='))
        .filter(|(key, _)| *key == name)
        .map(|(_, value)| value);
    let value = values.next()?;
    values.next().is_none().then_some(value)
}
fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|c| c.is_ascii_hexdigit())
}
fn digest(secret: &str, scope: &str, value: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hmac::sign(&key, format!("{scope}\0{value}").as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
async fn secret(pool: &PgPool) -> Result<String> {
    if let Some(secret) = sqlx::query_scalar("SELECT secret FROM report_abuse_settings WHERE id=1")
        .fetch_optional(pool)
        .await?
    {
        return Ok(secret);
    }
    sqlx::query(
        "INSERT INTO report_abuse_settings(id,secret) VALUES (1,$1) ON CONFLICT(id) DO NOTHING",
    )
    .bind(auth::token())
    .execute(pool)
    .await?;
    Ok(
        sqlx::query_scalar("SELECT secret FROM report_abuse_settings WHERE id=1")
            .fetch_one(pool)
            .await?,
    )
}
fn browser_cookie(headers: &HeaderMap, secret: &str, now: i64) -> Option<String> {
    let value = cookie(headers, COOKIE)?;
    let mut parts = value.split('.');
    let identity = parts.next()?;
    let issued = parts.next()?;
    let signature = parts.next()?;
    if parts.next().is_some() || !is_hex(identity, 64) || !is_hex(signature, 64) {
        return None;
    }
    let issued_at = issued.parse::<i64>().ok()?;
    if issued_at > now + 60 || issued_at < now - COOKIE_SECONDS {
        return None;
    }
    let signature: Vec<u8> = (0..64)
        .step_by(2)
        .map(|offset| u8::from_str_radix(&signature[offset..offset + 2], 16).ok())
        .collect::<Option<_>>()?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hmac::verify(
        &key,
        format!("cookie\0{identity}.{issued}").as_bytes(),
        &signature,
    )
    .ok()?;
    Some(identity.to_owned())
}
fn rate_error() -> Error {
    Error(
        StatusCode::TOO_MANY_REQUESTS,
        "V krátké době přišlo příliš mnoho požadavků. Zkuste to prosím za chvíli.",
    )
}
async fn rate(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    identity: &str,
    limit: i64,
    seconds: i64,
    now: i64,
) -> Result<()> {
    let bucket = now / seconds;
    let hits: i64 = sqlx::query_scalar("INSERT INTO report_abuse_rates(scope,identity_hash,bucket,hits,expires_at) VALUES ($1,$2,$3,1,$4) ON CONFLICT(scope,identity_hash,bucket) DO UPDATE SET hits=report_abuse_rates.hits+1 RETURNING hits")
        .bind(scope).bind(identity).bind(bucket).bind((bucket+1)*seconds).fetch_one(&mut **tx).await?;
    if hits > limit {
        return Err(rate_error());
    }
    Ok(())
}

pub async fn submission_token(
    State(state): State<Backend>,
    auth::ClientIp(ip): auth::ClientIp,
    headers: HeaderMap,
) -> Result<Response> {
    check_origin(&headers, &state)?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let secret = secret(&state.pool).await?;
    let existing = browser_cookie(&headers, &secret, now);
    let identity = existing.clone().unwrap_or_else(auth::token);
    let browser_hash = digest(&secret, "browser", &identity);
    let ip_hash = digest(&secret, "ip", &ip);
    let token = auth::token();
    let mut tx = crate::db::begin_write(&state.pool).await?;
    rate(&mut tx, "token-global", "all", 1200, 60, now).await?;
    rate(&mut tx, "token-ip", &ip_hash, 120, 600, now).await?;
    rate(&mut tx, "token-ip-day", &ip_hash, 1200, 86400, now).await?;
    rate(&mut tx, "token-browser", &browser_hash, 30, 600, now).await?;
    sqlx::query("DELETE FROM report_submission_tokens WHERE retain_until<$1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM report_abuse_rates WHERE expires_at<$1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO report_submission_tokens(token_hash,browser_hash,issuance_ip_hash,expires_at,retain_until) VALUES ($1,$2,$3,$4,$4)")
        .bind(auth::hash(&token)).bind(&browser_hash).bind(&ip_hash).bind(now+TOKEN_SECONDS).execute(&mut *tx).await?;
    tx.commit().await?;
    let mut response =
        Json(serde_json::json!({"token":token,"expires_at":now+TOKEN_SECONDS})).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    // Only create a cookie when needed, keeping simultaneous tabs on one identity.
    if existing.is_none() {
        let value = format!("{identity}.{now}");
        let signed = format!("{value}.{}", digest(&secret, "cookie", &value));
        let secure = if state.config.public_url.starts_with("https://") {
            "; Secure"
        } else {
            ""
        };
        response.headers_mut().insert(header::SET_COOKIE, format!("{COOKIE}={signed}; HttpOnly; SameSite=Strict; Path=/; Max-Age={COOKIE_SECONDS}{secure}").parse().unwrap());
    }
    Ok(response)
}

pub struct SubmissionContext {
    pub browser_hash: String,
    pub ip_hash: String,
    pub secret: String,
    pub persistent_browser: bool,
    pub header_token: Option<String>,
}
/// Run before consuming multipart bytes. Shared networks have deliberately broad limits.
pub async fn begin(
    state: &Backend,
    headers: &HeaderMap,
    ip: &str,
    now: i64,
) -> SubmissionResult<SubmissionContext> {
    check_origin(headers, state)?;
    let secret = secret(&state.pool).await?;
    let ip_hash = digest(&secret, "ip", ip);
    let browser = browser_cookie(headers, &secret, now);
    let raw_cookie_present = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|value| value.split(';'))
        .any(|part| part.trim().starts_with(&format!("{COOKIE}=")));
    let header_token = headers
        .get("x-report-token")
        .and_then(|v| v.to_str().ok())
        .filter(|v| is_hex(v, 64))
        .map(str::to_owned);
    let mut persistent_browser = browser.is_some();
    let mut tx = crate::db::begin_write(&state.pool).await?;
    rate(&mut tx, "attempt-ip", &ip_hash, 300, 600, now).await?;
    rate(&mut tx, "attempt-ip-day", &ip_hash, 2000, 86400, now).await?;
    let browser_hash = browser.map(|identity| digest(&secret, "browser", &identity));
    if let Some(hash) = &browser_hash {
        rate(&mut tx, "attempt-browser", hash, 60, 600, now).await?
    }
    tx.commit().await?;
    // The unguessable proof can recover a completed request even if its cookie
    // disappeared or rotated. claim() still requires the exact original payload.
    let completed_browser: Option<String> = if let Some(token) = &header_token {
        sqlx::query_scalar("SELECT browser_hash FROM report_submission_tokens WHERE token_hash=$1 AND report_id IS NOT NULL AND retain_until>=$2")
            .bind(auth::hash(token)).bind(now).fetch_optional(&state.pool).await?
    } else {
        None
    };
    let browser_hash = match (completed_browser, browser_hash) {
        (Some(hash), _) => {
            persistent_browser = false;
            hash
        }
        (None, Some(hash)) => hash,
        (None, None) if !raw_cookie_present => {
            // Without cookies the per-form secret is a short-lived bearer proof.
            // Require a custom same-origin header and the issuance network for new reports.
            // A completed request can still be retried after the network changes.
            let token = header_token
                .as_deref()
                .ok_or(SubmissionError::InvalidToken)?;
            sqlx::query_scalar("SELECT browser_hash FROM report_submission_tokens WHERE token_hash=$1 AND (issuance_ip_hash=$2 OR report_id IS NOT NULL) AND retain_until>=$3 AND (expires_at>=$3 OR report_id IS NOT NULL)")
                .bind(auth::hash(token)).bind(&ip_hash).bind(now).fetch_optional(&state.pool).await?
                .ok_or(SubmissionError::InvalidToken)?
        }
        (None, None) => return Err(SubmissionError::InvalidToken),
    };
    Ok(SubmissionContext {
        browser_hash,
        ip_hash,
        secret,
        persistent_browser,
        header_token,
    })
}

pub async fn token_valid(
    pool: &PgPool,
    context: &SubmissionContext,
    token: &str,
    now: i64,
) -> SubmissionResult<()> {
    if !is_hex(token, 64)
        || (!context.persistent_browser && context.header_token.as_deref() != Some(token))
    {
        return Err(SubmissionError::InvalidToken);
    }
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM report_submission_tokens WHERE token_hash=$1 AND browser_hash=$2 AND retain_until>=$3 AND (expires_at>=$3 OR report_id IS NOT NULL))")
        .bind(auth::hash(token)).bind(&context.browser_hash).bind(now).fetch_one(pool).await?;
    if valid {
        Ok(())
    } else {
        Err(SubmissionError::InvalidToken)
    }
}

/// Called inside the same serialized transaction as the report, photos and mail.
/// Returns an existing report for exact retries without consuming another quota.
pub async fn claim(
    tx: &mut Transaction<'_, Postgres>,
    context: &SubmissionContext,
    token: &str,
    payload: &str,
    now: i64,
) -> SubmissionResult<Option<i64>> {
    let row: Option<(i64,i64,Option<String>,Option<i64>)> = sqlx::query_as("SELECT expires_at,retain_until,payload_hash,report_id FROM report_submission_tokens WHERE token_hash=$1 AND browser_hash=$2 FOR UPDATE")
        .bind(auth::hash(token)).bind(&context.browser_hash).fetch_optional(&mut **tx).await?;
    let (expires, retain, prior_payload, report_id) = row.ok_or(SubmissionError::InvalidToken)?;
    if retain < now {
        return Err(SubmissionError::InvalidToken);
    }
    if let Some(report_id) = report_id {
        return if prior_payload.as_deref() == Some(payload) {
            Ok(Some(report_id))
        } else {
            Err(super::conflict("Toto hlášení už bylo odesláno. Upravený obsah nelze odeslat znovu stejným formulářem.").into())
        };
    }
    if expires < now {
        return Err(SubmissionError::InvalidToken);
    }
    let duplicate: Option<i64> = sqlx::query_scalar("SELECT report_id FROM report_submission_duplicates WHERE browser_hash=$1 AND payload_hash=$2 AND expires_at>=$3")
        .bind(&context.browser_hash).bind(payload).bind(now).fetch_optional(&mut **tx).await?;
    if let Some(id) = duplicate {
        finish_token(tx, token, payload, id, now).await?;
        return Ok(Some(id));
    }
    let (browser_recent,browser_day,ip_recent,ip_day): (i64,i64,i64,i64) = sqlx::query_as("SELECT count(*) FILTER(WHERE browser_hash=$1 AND created_at>$3-600),count(*) FILTER(WHERE browser_hash=$1),count(*) FILTER(WHERE ip_hash=$2 AND created_at>$3-600),count(*) FILTER(WHERE ip_hash=$2) FROM report_abuse_events WHERE created_at>$3-86400 AND (browser_hash=$1 OR ip_hash=$2)")
        .bind(&context.browser_hash).bind(&context.ip_hash).bind(now).fetch_one(&mut **tx).await?;
    if browser_recent >= 10 || browser_day >= 50 || ip_recent >= 100 || ip_day >= 500 {
        return Err(rate_error().into());
    }
    Ok(None)
}
async fn finish_token(
    tx: &mut Transaction<'_, Postgres>,
    token: &str,
    payload: &str,
    id: i64,
    now: i64,
) -> Result<()> {
    sqlx::query("UPDATE report_submission_tokens SET payload_hash=$1,report_id=$2,retain_until=$3 WHERE token_hash=$4")
        .bind(payload).bind(id).bind(now+RETRY_SECONDS).bind(auth::hash(token)).execute(&mut **tx).await?;
    Ok(())
}
pub async fn record(
    tx: &mut Transaction<'_, Postgres>,
    context: &SubmissionContext,
    token: &str,
    payload: &str,
    id: i64,
    now: i64,
) -> Result<()> {
    finish_token(tx, token, payload, id, now).await?;
    sqlx::query("INSERT INTO report_submission_duplicates(browser_hash,payload_hash,report_id,expires_at) VALUES ($1,$2,$3,$4) ON CONFLICT(browser_hash,payload_hash) DO UPDATE SET report_id=EXCLUDED.report_id,expires_at=EXCLUDED.expires_at")
        .bind(&context.browser_hash).bind(payload).bind(id).bind(now+RETRY_SECONDS).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO report_abuse_events(report_id,browser_hash,ip_hash,created_at) VALUES($1,$2,$3,$4)")
        .bind(id).bind(&context.browser_hash).bind(&context.ip_hash).bind(now).execute(&mut **tx).await?;
    Ok(())
}

pub fn bounded_header(headers: &HeaderMap, name: &str, max: usize) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .chars()
                .filter(|c| !c.is_control())
                .take(max)
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
}
pub fn timezone(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_+-".contains(&c))
    {
        return Err(bad("Neplatný údaj o časovém pásmu."));
    }
    Ok((!value.is_empty()).then(|| value.to_owned()))
}
fn browser_family(agent: &str) -> String {
    for marker in ["Edg/", "Firefox/", "Chrome/", "Version/"] {
        if let Some(version) = agent
            .split_once(marker)
            .map(|(_, after)| after.split('.').next().unwrap_or_default())
        {
            let major: String = version
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .take(3)
                .collect();
            return format!("{marker}{major}");
        }
    }
    "other".to_owned()
}
pub fn fingerprint(
    context: &SubmissionContext,
    agent: Option<&str>,
    language: Option<&str>,
    zone: Option<&str>,
) -> String {
    let agent = agent.unwrap_or_default();
    let os = ["Android", "iPhone", "iPad", "Windows", "Macintosh", "Linux"]
        .into_iter()
        .find(|os| agent.contains(os))
        .unwrap_or("other");
    let language = language
        .unwrap_or_default()
        .split([',', ';', '-'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    digest(
        &context.secret,
        "fingerprint",
        &format!(
            "{}\0{os}\0{language}\0{}",
            browser_family(agent),
            zone.unwrap_or_default()
        ),
    )
}
pub async fn authenticated_sender(
    pool: &PgPool,
    headers: &HeaderMap,
    now: i64,
) -> Result<Option<(i64, String)>> {
    let Some(session) = cookie(headers, "obec_session").filter(|s| is_hex(s, 64)) else {
        return Ok(None);
    };
    Ok(sqlx::query_as("SELECT a.id,a.email FROM sessions s JOIN administrators a ON a.id=s.administrator_id WHERE s.token_hash=$1 AND s.expires_at>$2 AND a.active=TRUE")
        .bind(auth::hash(session)).bind(now).fetch_optional(pool).await?)
}
pub async fn maintenance(pool: &PgPool, now: i64) -> Result<()> {
    let mut tx = crate::db::begin_write(pool).await?;
    sqlx::query("UPDATE issue_reports SET request_ip='',request_user_agent=NULL,request_accept_language=NULL,client_timezone=NULL,browser_id_hash=NULL,browser_fingerprint_hash=NULL,authenticated_admin_id=NULL,authenticated_admin_email=NULL WHERE metadata_expires_at<=$1 AND (request_ip<>'' OR request_user_agent IS NOT NULL OR request_accept_language IS NOT NULL OR client_timezone IS NOT NULL OR browser_id_hash IS NOT NULL OR browser_fingerprint_hash IS NOT NULL OR authenticated_admin_id IS NOT NULL OR authenticated_admin_email IS NOT NULL)")
        .bind(now).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM report_submission_tokens WHERE retain_until<$1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM report_submission_duplicates WHERE expires_at<$1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM report_abuse_rates WHERE expires_at<$1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM report_abuse_events WHERE created_at<$1-86400")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
