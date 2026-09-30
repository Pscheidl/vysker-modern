//! Single-use recovery tokens and a durable, separately retained security-mail queue.
use super::{Backend, Result, accounts, audit, auth, bad};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;
use time::OffsetDateTime;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    email: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reset {
    token: String,
    password: String,
}

pub async fn policy(State(s): State<Backend>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"minimum_password_length":s.config.minimum_password_length}))
}

pub async fn request(
    State(s): State<Backend>,
    auth::ClientIp(ip): auth::ClientIp,
    headers: HeaderMap,
    Json(input): Json<Request>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    auth::check_origin(&headers, &s)?;
    let now = OffsetDateTime::now_utc();
    auth::throttle(&s, &format!("recovery-ip:{ip}"), 20, 3600, now).await?;
    let email = auth::email(&input.email)?;
    // Rate-limit every address alike, without exposing whether an account exists.
    if let Err(error) = auth::throttle(&s, &format!("recovery-mail:{email}"), 3, 3600, now).await {
        if error.0 == StatusCode::TOO_MANY_REQUESTS {
            return Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"accepted":true})),
            ));
        }
        return Err(error);
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM administrators WHERE email=$1 AND active=TRUE")
            .bind(email)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(id) = id {
        let secret = auth::token();
        let hash = auth::hash(&secret);
        // A repeated request does not invalidate an earlier link. A successful
        // password change revokes every outstanding token for this account.
        sqlx::query(
            "INSERT INTO password_resets(hash,administrator_id,expires_at) VALUES ($1,$2,$3)",
        )
        .bind(&hash)
        .bind(id)
        .bind(now.unix_timestamp() + 1800)
        .execute(&mut *tx)
        .await?;
        let body = format!(
            "Dobrý den,\n\nobnovu hesla ke správě webu Vyskeř dokončíte zde:\n{}/admin/obnova#token={}\n\nOdkaz platí 30 minut a lze jej použít jednou. Pokud jste o změnu nepožádali, zprávu ignorujte.\n\nObec Vyskeř",
            s.config.public_url, secret
        );
        sqlx::query("INSERT INTO recovery_mail(administrator_id,token_hash,body,created_at,next_attempt_at) VALUES ($1,$2,$3,$4,$4)")
            .bind(id).bind(hash).bind(body).bind(now.unix_timestamp()).execute(&mut *tx).await?;
        audit(
            &mut tx,
            None,
            "password_recovery_requested",
            "administrator",
            id,
            now,
        )
        .await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"accepted":true})),
    ))
}

pub async fn reset(
    State(s): State<Backend>,
    auth::ClientIp(ip): auth::ClientIp,
    headers: HeaderMap,
    Json(input): Json<Reset>,
) -> Result<StatusCode> {
    auth::check_origin(&headers, &s)?;
    let now = OffsetDateTime::now_utc();
    auth::throttle(&s, &format!("reset-ip:{ip}"), 20, 900, now).await?;
    if input.token.len() != 64 || !input.token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad("Odkaz není platný nebo již vypršel."));
    }
    let hash = auth::hash(&input.token);
    let eligible: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM password_resets r JOIN administrators a ON a.id=r.administrator_id WHERE r.hash=$1 AND r.used_at IS NULL AND r.expires_at>$2 AND a.active=TRUE)")
        .bind(&hash).bind(now.unix_timestamp()).fetch_one(&s.pool).await?;
    if !eligible {
        return Err(bad("Odkaz není platný nebo již vypršel."));
    }
    let password = auth::password_hash(input.password, s.config.minimum_password_length).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id: Option<i64> = sqlx::query_scalar("UPDATE password_resets SET used_at=$1 WHERE hash=$2 AND used_at IS NULL AND expires_at>$1 AND EXISTS(SELECT 1 FROM administrators WHERE id=password_resets.administrator_id AND active=TRUE) RETURNING administrator_id")
        .bind(OffsetDateTime::now_utc().unix_timestamp()).bind(hash).fetch_optional(&mut *tx).await?;
    let id = id.ok_or_else(|| bad("Odkaz není platný nebo již vypršel."))?;
    accounts::set_password(&mut tx, id, password, Some(id), "password_recovered").await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn invalidate(conn: &mut sqlx::PgConnection, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM password_resets WHERE administrator_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE recovery_mail SET cancelled=TRUE,body=NULL WHERE administrator_id=$1 AND sent_at IS NULL")
        .bind(id).execute(conn).await?;
    Ok(())
}

pub async fn maintenance(s: &Backend, now: OffsetDateTime) -> Result<()> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    sqlx::query("UPDATE recovery_mail SET delivery_expired_at=CASE WHEN cancelled=FALSE AND token_hash IN (SELECT hash FROM password_resets WHERE expires_at<=$1 AND used_at IS NULL) THEN $1 ELSE delivery_expired_at END,cancelled=TRUE,body=NULL WHERE sent_at IS NULL AND (token_hash IS NULL OR token_hash IN (SELECT hash FROM password_resets WHERE expires_at<=$1 OR used_at IS NOT NULL))")
        .bind(now.unix_timestamp()).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM password_resets WHERE expires_at<=$1 OR used_at IS NOT NULL")
        .bind(now.unix_timestamp())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM recovery_mail WHERE created_at<$1 AND locked_until<=$2")
        .bind(now.unix_timestamp() - 30 * 86400)
        .bind(now.unix_timestamp())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Pending {
    id: i64,
    administrator_id: i64,
    token_hash: Option<String>,
    body: Option<String>,
    attempts: i64,
}
pub async fn deliver_one(
    s: &Backend,
    smtp: &AsyncSmtpTransport<Tokio1Executor>,
    now: OffsetDateTime,
) -> anyhow::Result<bool> {
    let lease = auth::token();
    let message:Option<Pending>=sqlx::query_as("UPDATE recovery_mail SET lock_token=$1,locked_until=$2,attempts=attempts+1 WHERE id=(SELECT id FROM recovery_mail WHERE sent_at IS NULL AND cancelled=FALSE AND next_attempt_at<=$3 AND locked_until<=$3 ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,administrator_id,token_hash,body,attempts")
        .bind(&lease).bind(now.unix_timestamp()+300).bind(now.unix_timestamp()).fetch_optional(&s.pool).await?;
    let Some(message) = message else {
        return Ok(false);
    };
    let address:Option<String>=sqlx::query_scalar("SELECT a.email FROM administrators a JOIN password_resets r ON r.administrator_id=a.id JOIN recovery_mail m ON m.token_hash=r.hash WHERE a.id=$1 AND a.active=TRUE AND r.hash=$2 AND r.used_at IS NULL AND r.expires_at>$3 AND m.id=$4 AND m.cancelled=FALSE AND m.lock_token=$5")
        .bind(message.administrator_id).bind(message.token_hash).bind(now.unix_timestamp()).bind(message.id).bind(&lease).fetch_optional(&s.pool).await?;
    let Some((address, body)) = address.zip(message.body) else {
        sqlx::query("UPDATE recovery_mail SET delivery_expired_at=CASE WHEN cancelled=FALSE AND token_hash IN (SELECT hash FROM password_resets WHERE expires_at<=$3 AND used_at IS NULL) THEN $3 ELSE delivery_expired_at END,cancelled=TRUE,body=NULL,lock_token=NULL,locked_until=0 WHERE id=$1 AND lock_token=$2").bind(message.id).bind(lease).bind(now.unix_timestamp()).execute(&s.pool).await?;
        return Ok(true);
    };
    let outcome = async {
        let message_id = format!(
            "vysker-recovery-{}@{}",
            message.id,
            url::Url::parse(&s.config.public_url)?
                .host_str()
                .unwrap_or("localhost")
        );
        let email = Message::builder()
            .from(s.config.email_from.parse()?)
            .to(address.parse()?)
            .message_id(Some(message_id))
            .subject("Obnova hesla správy webu Vyskeř")
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(body)?;
        smtp.send(email).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if outcome.is_ok() {
        sqlx::query("UPDATE recovery_mail SET sent_at=$1,body=NULL,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3")
            .bind(now.unix_timestamp()).bind(message.id).bind(lease).execute(&s.pool).await?;
    } else {
        let delay = (30_i64 * 2_i64.pow(message.attempts.min(10) as u32)).min(21600);
        sqlx::query("UPDATE recovery_mail SET next_attempt_at=$1,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3")
            .bind(now.unix_timestamp()+delay).bind(message.id).bind(lease).execute(&s.pool).await?;
        tracing::warn!(
            message_id = message.id,
            "Recovery SMTP failed, retry scheduled"
        );
    }
    Ok(true)
}
