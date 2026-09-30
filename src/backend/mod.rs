//! Backendové API. SQL, hesla a SMTP jsou dostupné výhradně ve feature ssr.
pub mod accounts;
pub mod auth;
pub mod documents;
pub mod mail;
pub mod management;
pub mod notices;
pub mod pages;
pub mod privacy;
pub mod search;
pub mod server;
pub mod subscriptions;

use crate::config::Config;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use std::sync::Arc;
use time::{Date, OffsetDateTime};
use time_tz::OffsetDateTimeExt;

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct Backend {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub last_maintenance: Arc<std::sync::atomic::AtomicI64>,
}

impl Backend {
    pub fn new(pool: PgPool, config: Config) -> Self {
        Self {
            pool,
            config: Arc::new(config),
            last_maintenance: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        }
    }
}

#[derive(Debug)]
pub struct Error(pub StatusCode, pub &'static str);
pub type Result<T> = std::result::Result<T, Error>;
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error": self.1}))).into_response()
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            return conflict("Záznam s touto hodnotou už existuje.");
        }
        tracing::error!(%error, "databázová operace selhala");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Operaci se nepodařilo dokončit.",
        )
    }
}
pub fn bad(message: &'static str) -> Error {
    Error(StatusCode::BAD_REQUEST, message)
}
pub fn conflict(message: &'static str) -> Error {
    Error(StatusCode::CONFLICT, message)
}
pub fn missing() -> Error {
    Error(StatusCode::NOT_FOUND, "Záznam není dostupný.")
}
pub fn today(now: OffsetDateTime) -> Date {
    now.to_timezone(time_tz::timezones::db::europe::PRAGUE)
        .date()
}
pub fn timestamp(now: OffsetDateTime) -> String {
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("valid UTC timestamp")
}
pub fn text(value: &str, max: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > max
        || value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(bad(
            "Text je prázdný, příliš dlouhý nebo obsahuje nepovolené znaky.",
        ));
    }
    Ok(value.into())
}
pub async fn audit(
    conn: &mut PgConnection,
    actor: Option<i64>,
    operation: &str,
    kind: &str,
    id: i64,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_log(occurred_at,actor_id,operation,entity_type,entity_id) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(timestamp(now))
    .bind(actor)
    .bind(operation)
    .bind(kind)
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Deserialize, Default)]
pub struct Pagination {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
impl Pagination {
    pub fn bounds(&self) -> Result<(i64, i64)> {
        let (limit, offset) = (self.limit.unwrap_or(50), self.offset.unwrap_or(0));
        if !(1..=100).contains(&limit) || !(0..=100_000).contains(&offset) {
            return Err(bad("Neplatné stránkování."));
        }
        Ok((limit, offset))
    }
}
#[derive(sqlx::FromRow, Serialize)]
struct AuditRecord {
    id: i64,
    occurred_at: String,
    actor_id: Option<i64>,
    operation: String,
    entity_type: String,
    entity_id: Option<i64>,
    actor_email: Option<String>,
}
async fn audit_list(
    State(s): State<Backend>,
    _: auth::Admin,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<AuditRecord>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(sqlx::query_as("SELECT l.id,l.occurred_at,l.actor_id,l.operation,l.entity_type,l.entity_id,a.email AS actor_email FROM audit_log l LEFT JOIN administrators a ON a.id=l.actor_id ORDER BY l.id DESC LIMIT $1 OFFSET $2")
        .bind(limit).bind(offset).fetch_all(&s.pool).await?))
}
async fn health(State(s): State<Backend>) -> Result<Json<serde_json::Value>> {
    sqlx::query("SELECT 1").execute(&s.pool).await?;
    Ok(Json(serde_json::json!({"status":"ok"})))
}

pub fn router(state: Backend) -> Router {
    let config = state.config.clone();
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/search", get(search::endpoint))
        .route("/api/v1/ready", get(ready))
        .route("/api/v1/admin/mail", get(management::mail_history))
        .route("/api/v1/privacy", get(privacy::public_notice))
        .route(
            "/api/v1/admin/subscribers/{id}/consents",
            get(privacy::evidence),
        )
        .route(
            "/api/v1/admin/accounts",
            get(accounts::list).post(accounts::create),
        )
        .route(
            "/api/v1/admin/accounts/{id}",
            axum::routing::put(accounts::set_active),
        )
        .route(
            "/api/v1/admin/accounts/{id}/sessions",
            axum::routing::delete(accounts::revoke_sessions),
        )
        .route(
            "/api/v1/admin/password",
            axum::routing::put(accounts::change_password),
        )
        .route("/api/v1/admin/login", post(auth::login))
        .route(
            "/api/v1/admin/session",
            get(auth::session).delete(auth::logout),
        )
        .route("/api/v1/admin/audit", get(audit_list))
        .route("/api/v1/admin/overview", get(management::overview))
        .route(
            "/api/v1/admin/notices",
            get(management::notices).post(notices::create),
        )
        .route(
            "/api/v1/admin/notices/{id}",
            get(management::notice).put(notices::update),
        )
        .route("/api/v1/admin/notices/{id}/publish", post(notices::publish))
        .route(
            "/api/v1/admin/notices/{id}/withdraw",
            post(notices::withdraw),
        )
        .route(
            "/api/v1/admin/notices/{id}/attachments",
            post(documents::upload_notice).layer(DefaultBodyLimit::max(MAX_FILE_BYTES + 64 * 1024)),
        )
        .route(
            "/api/v1/admin/notices/{id}/evidence",
            get(notices::publication_evidence),
        )
        .route(
            "/api/v1/admin/notices/{id}/incidents",
            post(notices::incident),
        )
        .route("/api/v1/notices", get(notices::public_list))
        .route("/api/v1/notices/{id}", get(notices::detail))
        .route("/api/v1/categories", get(notices::categories))
        .route(
            "/api/v1/admin/documents",
            get(management::documents).post(documents::create),
        )
        .route(
            "/api/v1/admin/documents/{id}",
            get(management::document).put(documents::update),
        )
        .route(
            "/api/v1/admin/attachments/{id}",
            get(management::download).delete(management::remove_file),
        )
        .route(
            "/api/v1/admin/documents/{id}/publish",
            post(documents::publish),
        )
        .route(
            "/api/v1/admin/documents/{id}/archive",
            post(documents::archive),
        )
        .route(
            "/api/v1/admin/documents/{id}/attachments",
            post(documents::upload_document)
                .layer(DefaultBodyLimit::max(MAX_FILE_BYTES + 64 * 1024)),
        )
        .route("/api/v1/documents", get(documents::public_list))
        .route("/api/v1/documents/{id}", get(documents::detail))
        .route("/api/v1/attachments/{id}", get(documents::download))
        .route(
            "/api/v1/admin/pages",
            get(management::pages)
                .post(pages::create)
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route(
            "/api/v1/admin/pages/{id}",
            get(management::page)
                .put(pages::update)
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/api/v1/pages/{slug}", get(pages::detail))
        .route("/api/v1/subscriptions", post(subscriptions::subscribe))
        .route("/api/v1/subscriptions/verify", post(subscriptions::confirm))
        .route(
            "/api/v1/subscriptions/unsubscribe",
            post(subscriptions::unsubscribe),
        )
        .route(
            "/odber/potvrdit",
            get(subscriptions::confirmation_page).post(subscriptions::confirmation_form),
        )
        .route(
            "/odber/odhlasit",
            get(subscriptions::unsubscribe_page).post(subscriptions::unsubscribe_form),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(axum::middleware::from_fn(auth::response_headers))
        .layer(axum::middleware::from_fn_with_state(
            config,
            privacy::response_headers,
        ))
        .with_state(state)
}

async fn ready(State(s): State<Backend>) -> Result<Json<serde_json::Value>> {
    sqlx::query("SELECT 1").execute(&s.pool).await?;
    let age = OffsetDateTime::now_utc().unix_timestamp()
        - s.last_maintenance
            .load(std::sync::atomic::Ordering::Relaxed);
    if !(0..=120).contains(&age) {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Údržba webu není připravená.",
        ));
    }
    Ok(Json(serde_json::json!({"status":"ready"})))
}
