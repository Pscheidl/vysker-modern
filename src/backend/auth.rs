use super::{Backend, Error, Result, audit, bad};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, Method, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use time::OffsetDateTime;

pub fn token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn hash(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value.as_ref()))
}
pub fn email(value: &str) -> Result<String> {
    let value = value.trim().to_lowercase();
    if value.len() > 254
        || value.contains(['\r', '\n', ' ', '<', '>'])
        || value.parse::<lettre::Address>().is_err()
    {
        return Err(bad("Zadejte platnou e-mailovou adresu."));
    }
    Ok(value)
}
pub(super) fn denied() -> Error {
    Error(
        StatusCode::UNAUTHORIZED,
        "Přihlášení je neplatné nebo vypršelo.",
    )
}

pub async fn password_hash(password: String, minimum: usize) -> Result<String> {
    if password.chars().count() < minimum || password.len() > 1024 {
        return Err(bad(
            "Heslo nesplňuje nastavenou minimální délku nebo přesahuje 1024 bajtů.",
        ));
    }
    let password_hash = tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
            .map(|h| h.to_string())
    })
    .await
    .map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Vytvoření hesla selhalo.",
        )
    })?
    .map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Vytvoření hesla selhalo.",
        )
    })?;
    Ok(password_hash)
}

pub(super) async fn verify_password(password: String, stored: String) -> Result<()> {
    if password.len() > 1024 {
        return Err(denied());
    }
    let valid = tokio::task::spawn_blocking(move || {
        PasswordHash::new(&stored).ok().is_some_and(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
    })
    .await
    .map_err(|_| denied())?;
    if valid { Ok(()) } else { Err(denied()) }
}

pub async fn create_admin(s: &Backend, address: &str, password: String) -> Result<i64> {
    let address = email(address)?;
    let password_hash = password_hash(password, s.config.minimum_password_length).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO administrators(email,password_hash) VALUES ($1,$2) RETURNING id",
    )
    .bind(address)
    .bind(password_hash)
    .fetch_one(&mut *tx)
    .await?;
    audit(
        &mut tx,
        None,
        "created",
        "administrator",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub fn check_origin(headers: &HeaderMap, s: &Backend) -> Result<()> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        if origin.to_str().ok() != Some(s.config.public_url.as_str()) {
            return Err(Error(
                StatusCode::FORBIDDEN,
                "Požadavek pochází z jiného webu.",
            ));
        }
    }
    Ok(())
}

/// Bez X-Forwarded-For. Hlavičce od neověřeného proxy nesmíme věřit.
#[derive(Clone)]
pub struct ClientIp(pub String);
impl FromRequestParts<Backend> for ClientIp {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, state: &Backend) -> Result<Self> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|v| v.0.ip());
        Ok(Self(client_ip(
            peer,
            &parts.headers,
            state.config.trusted_proxy,
        )))
    }
}
pub fn client_ip(
    peer: Option<std::net::IpAddr>,
    headers: &HeaderMap,
    trusted: Option<std::net::IpAddr>,
) -> String {
    if peer.is_some() && peer == trusted {
        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<std::net::IpAddr>().ok())
        {
            return ip.to_string();
        }
    }
    peer.map(|ip| ip.to_string())
        .unwrap_or_else(|| "local".into())
}

pub async fn throttle(
    s: &Backend,
    key: &str,
    limit: i64,
    seconds: i64,
    now: OffsetDateTime,
) -> Result<()> {
    let count: i64 = sqlx::query_scalar(
        "INSERT INTO rate_limits(key,window_started_at,count) VALUES ($1,$2,1)
        ON CONFLICT(key) DO UPDATE SET count=CASE WHEN rate_limits.window_started_at <= $3 THEN 1 ELSE rate_limits.count+1 END,
        window_started_at=CASE WHEN rate_limits.window_started_at <= $4 THEN excluded.window_started_at ELSE rate_limits.window_started_at END RETURNING count",
    )
    .bind(hash(key))
    .bind(now.unix_timestamp())
    .bind(now.unix_timestamp() - seconds)
    .bind(now.unix_timestamp() - seconds)
    .fetch_one(&s.pool)
    .await?;
    if count > limit {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "Příliš mnoho pokusů. Zkuste to později.",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Login {
    pub email: String,
    pub password: String,
}
#[derive(Serialize)]
pub struct Session {
    pub administrator_id: i64,
    pub email: String,
    pub csrf_token: String,
    pub minimum_password_length: usize,
}

pub async fn login(
    State(s): State<Backend>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response> {
    check_origin(&headers, &s)?;
    let now = OffsetDateTime::now_utc();
    throttle(&s, &format!("login-ip:{ip}"), 30, 900, now).await?;
    let address = email(&input.email)?;
    throttle(&s, &format!("login-mail:{address}"), 10, 900, now).await?;
    if input.password.len() > 1024 {
        return Err(denied());
    }
    let user: Option<(i64, String)> = sqlx::query_as(
        "SELECT id,password_hash FROM administrators WHERE email=$1 AND active=TRUE",
    )
    .bind(&address)
    .fetch_optional(&s.pool)
    .await?;
    let id = user.as_ref().map(|u| u.0);
    let authenticated_hash = user.as_ref().map(|u| u.1.clone());
    let valid = tokio::task::spawn_blocking(move || {
        if let Some((_, stored)) = user {
            PasswordHash::new(&stored).ok().is_some_and(|h| {
                Argon2::default()
                    .verify_password(input.password.as_bytes(), &h)
                    .is_ok()
            })
        } else {
            let _ = Argon2::default()
                .hash_password(input.password.as_bytes(), &SaltString::generate(&mut OsRng));
            false
        }
    })
    .await
    .map_err(|_| denied())?;
    if !valid {
        return Err(denied());
    }
    let id = id.ok_or_else(denied)?;
    let secret = token();
    let csrf = hash(format!("csrf:{secret}"));
    let mut tx = crate::db::begin_write(&s.pool).await?;
    // Serialize with password changes and deactivation. Authentication outside the
    // write transaction must not create a session for a superseded password.
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM administrators WHERE id=$1 AND active=TRUE AND password_hash=$2)",
    )
    .bind(id)
    .bind(authenticated_hash)
    .fetch_one(&mut *tx)
    .await?;
    if !current {
        return Err(denied());
    }
    sqlx::query(
        "INSERT INTO sessions(token_hash,csrf_hash,administrator_id,expires_at) VALUES ($1,$2,$3,$4)",
    )
    .bind(hash(&secret))
    .bind(hash(&csrf))
    .bind(id)
    .bind(now.unix_timestamp() + 8 * 3600)
    .execute(&mut *tx)
    .await?;
    audit(&mut tx, Some(id), "logged_in", "administrator", id, now).await?;
    tx.commit().await?;
    let cookie = format!(
        "obec_session={secret}; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800{}",
        if s.config.public_url.starts_with("https:") {
            "; Secure"
        } else {
            ""
        }
    );
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(Session {
            administrator_id: id,
            email: address,
            csrf_token: csrf,
            minimum_password_length: s.config.minimum_password_length,
        }),
    )
        .into_response())
}

pub struct Admin {
    pub id: i64,
    pub email: String,
    pub(super) token_hash: String,
    csrf: String,
}
impl FromRequestParts<Backend> for Admin {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, s: &Backend) -> Result<Self> {
        let secret = parts
            .headers
            .get(header::COOKIE)
            .and_then(|h| h.to_str().ok())
            .and_then(|cookies| {
                cookies
                    .split(';')
                    .find_map(|c| c.trim().strip_prefix("obec_session="))
            })
            .ok_or_else(denied)?;
        if secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(denied());
        }
        let token_hash = hash(secret);
        let row: Option<(i64, String, String)> = sqlx::query_as("SELECT r.administrator_id,r.csrf_hash,a.email FROM sessions r JOIN administrators a ON a.id=r.administrator_id WHERE r.token_hash=$1 AND r.expires_at>$2 AND a.active=TRUE")
            .bind(&token_hash).bind(OffsetDateTime::now_utc().unix_timestamp()).fetch_optional(&s.pool).await?;
        let (id, csrf_hash, email) = row.ok_or_else(denied)?;
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
            check_origin(&parts.headers, s)?;
            let supplied = parts
                .headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if hash(supplied) != csrf_hash {
                return Err(Error(StatusCode::FORBIDDEN, "Chybí platný CSRF token."));
            }
        }
        Ok(Self {
            id,
            email,
            token_hash,
            csrf: hash(format!("csrf:{secret}")),
        })
    }
}
pub async fn session(State(s): State<Backend>, admin: Admin) -> Json<Session> {
    Json(Session {
        administrator_id: admin.id,
        email: admin.email,
        csrf_token: admin.csrf,
        minimum_password_length: s.config.minimum_password_length,
    })
}
pub async fn logout(State(s): State<Backend>, admin: Admin) -> Result<Response> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
        .bind(admin.token_hash)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        Some(admin.id),
        "logged_out",
        "administrator",
        admin.id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok((
        [(
            header::SET_COOKIE,
            "obec_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}
pub async fn response_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    response
}

/// Neveřejné rozhraní se nesmí ukládat do sdílených cache ani zobrazit v rámu.
pub async fn admin_page_headers(request: Request, next: Next) -> Response {
    let is_admin = request.uri().path() == "/admin" || request.uri().path().starts_with("/admin/");
    let mut response = next.run(request).await;
    if is_admin {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        response
            .headers_mut()
            .insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            "frame-ancestors 'none'".parse().unwrap(),
        );
        response
            .headers_mut()
            .insert(header::REFERRER_POLICY, "same-origin".parse().unwrap());
    }
    response
}
