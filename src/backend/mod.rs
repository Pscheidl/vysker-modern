//! Backendové API. SQL, hesla a SMTP jsou dostupné výhradně ve feature ssr.
pub mod accounts;
pub mod agent_content;
pub mod agent_docs;
pub mod attachment_index;
pub mod attachment_text;
pub mod auth;
pub mod calendar;
pub mod cez;
pub mod documents;
pub mod events;
pub mod legacy;
pub mod mail;
pub mod mail_settings;
pub mod management;
pub mod mcp;
pub mod navigation;
pub mod notices;
pub mod page_images;
pub mod pages;
pub mod power_outages;
pub mod privacy;
pub mod recovery;
pub mod registry;
mod rental_invoice_mail;
mod rental_invoice_pdf;
mod rental_payment;
pub mod rental_payment_settings;
pub mod rental_settings;
pub mod rentals;
pub mod report_abuse;
pub mod report_addresses;
pub mod report_comments;
pub mod reports;
pub mod search;
pub mod server;
pub mod subscriber_admin;
pub mod subscription_preferences;
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
        if error.as_database_error().is_some_and(|e| {
            e.code().as_deref() == Some("23514")
                && e.constraint() == Some("document_lifecycle_guard")
        }) {
            return conflict(
                "Operace porušuje ochranu dokumentu nebo stav evidence. Načtěte aktuální záznam.",
            );
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
    let mcp_routes = mcp::router(state.clone());
    let discovery_routes = Router::new()
        .route(
            "/llms.txt",
            get(agent_docs::llms).options(agent_docs::options),
        )
        .route(
            "/for-agents.md",
            get(agent_docs::guide).options(agent_docs::options),
        )
        .route(
            "/openapi.json",
            get(agent_docs::openapi).options(agent_docs::options),
        )
        .route(
            "/.well-known/ai-catalog.json",
            get(agent_docs::catalog).options(agent_docs::options),
        )
        .route(
            "/mcp/server-card",
            get(agent_docs::server_card).options(agent_docs::options),
        )
        .layer(axum::middleware::from_fn_with_state(
            config.clone(),
            privacy::response_headers,
        ))
        .with_state(state.clone());
    let rental_routes = Router::new()
        .route("/api/v1/rental-assets", get(rentals::assets))
        .route(
            "/api/v1/rental-assets/{id}/availability",
            get(rentals::availability),
        )
        .route(
            "/api/v1/rentals",
            post(rentals::create).layer(DefaultBodyLimit::max(10 * 1024 * 1024 + 64 * 1024)),
        )
        .route("/api/v1/rental-access/{token}", get(rentals::access))
        .route(
            "/api/v1/rental-access/{token}/photos",
            post(rentals::upload).layer(DefaultBodyLimit::max(20 * 1024 * 1024 + 64 * 1024)),
        )
        .route(
            "/api/v1/rental-access/{token}/photos/{kind}",
            get(rentals::access_photo),
        )
        .route(
            "/api/v1/admin/rental-assets",
            get(rentals::admin_assets).post(rentals::create_asset),
        )
        .route(
            "/api/v1/admin/rental-assets/{id}",
            axum::routing::put(rentals::update_asset),
        )
        .route(
            "/api/v1/admin/rental-assets/{id}/blocks",
            get(rentals::list_blocks).post(rentals::create_block),
        )
        .route(
            "/api/v1/admin/rental-assets/{id}/blocks/{block_id}",
            axum::routing::delete(rentals::delete_block),
        )
        .route(
            "/api/v1/admin/rentals",
            get(rentals::admin_list)
                .post(rentals::create_admin_rental)
                .layer(DefaultBodyLimit::max(10 * 1024 * 1024 + 64 * 1024)),
        )
        .route(
            "/api/v1/admin/rentals/preview",
            post(rentals::preview_admin_rental),
        )
        .route(
            "/api/v1/admin/rentals/{id}/cancel",
            post(rentals::cancel_admin_rental),
        )
        .route(
            "/api/v1/admin/rental-series/{id}/cancel",
            post(rentals::cancel_series),
        )
        .route(
            "/api/v1/admin/rental-payment-settings",
            get(rental_payment_settings::read).put(rental_payment_settings::update),
        )
        .route("/api/v1/admin/rentals/{id}", get(rentals::admin_detail))
        .route(
            "/api/v1/admin/rentals/{id}/identity",
            post(rentals::identity).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/v1/admin/rentals/{id}/photos/{kind}",
            get(rentals::photo),
        )
        .route("/api/v1/admin/rentals/{id}/approve", post(rentals::approve))
        .route("/api/v1/admin/rentals/{id}/reject", post(rentals::reject))
        .route("/api/v1/admin/rentals/{id}/invoice", post(rentals::invoice))
        .route(
            "/api/v1/admin/rentals/{id}/invoice-with-attachment",
            post(rentals::invoice_with_attachment)
                .layer(DefaultBodyLimit::max(MAX_FILE_BYTES + 64 * 1024)),
        )
        .route("/api/v1/admin/rentals/{id}/paid", post(rentals::paid))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rental_settings::gate,
        ));
    Router::new()
        .merge(rental_routes)
        .route(
            "/api/v1/admin/rental-settings",
            get(rental_settings::read).put(rental_settings::update),
        )
        .route("/api/v1/legacy-media/{id}", get(legacy::media))
        .route("/api/v1/admin/legacy-media/{id}", get(legacy::admin_media))
        .route("/api/v1/page-images/{id}", get(page_images::media))
        .route(
            "/api/v1/admin/pages/{id}/images",
            get(page_images::list)
                .post(page_images::upload)
                .layer(DefaultBodyLimit::max(MAX_FILE_BYTES + 64 * 1024)),
        )
        .route("/api/v1/health", get(health))
        .route(
            "/api/v1/admin/events",
            get(events::list).post(events::create),
        )
        .route(
            "/api/v1/admin/events/{id}",
            get(events::get).put(events::update).delete(events::delete),
        )
        .route("/api/v1/events", get(events::public_list))
        .route("/api/v1/calendar.ics", get(calendar::feed))
        .route("/api/v1/events/{id}/calendar.ics", get(calendar::single))
        .route("/api/v1/report-addresses", get(report_addresses::search))
        .route(
            "/api/v1/report-submission-token",
            post(report_abuse::submission_token).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/api/v1/events/{id}", get(events::detail))
        .route(
            "/api/v1/reports",
            get(reports::public_list)
                .post(reports::create)
                .layer(DefaultBodyLimit::max(
                    crate::reports::REPORT_UPLOAD_BYTES + 64 * 1024,
                )),
        )
        .route("/api/v1/reports/map", get(reports::public_map))
        .route("/api/v1/reports/{id}", get(reports::detail))
        .route(
            "/api/v1/reports/{id}/comments",
            get(report_comments::public_list),
        )
        .route(
            "/api/v1/reports/{id}/comments/{comment_id}/photos/{photo_id}",
            get(report_comments::photo),
        )
        .route(
            "/api/v1/admin/reports/{id}/comments",
            get(report_comments::admin_list)
                .post(report_comments::create)
                .layer(DefaultBodyLimit::max(
                    crate::reports::REPORT_UPLOAD_BYTES + 64 * 1024,
                )),
        )
        .route(
            "/api/v1/admin/reports/{id}/comments/{comment_id}",
            axum::routing::put(report_comments::update)
                .delete(report_comments::delete)
                .layer(DefaultBodyLimit::max(
                    crate::reports::REPORT_UPLOAD_BYTES + 64 * 1024,
                )),
        )
        .route(
            "/api/v1/admin/reports/{id}/comments/{comment_id}/photos/{photo_id}",
            get(report_comments::admin_photo),
        )
        .route("/api/v1/reports/{id}/photo", get(reports::photo))
        .route(
            "/api/v1/reports/{id}/photos/{position}",
            get(reports::gallery_photo),
        )
        .route(
            "/api/v1/reports/{id}/resolution-photos/{photo_id}",
            get(reports::resolution_photo),
        )
        .route("/api/v1/admin/reports", get(reports::list))
        .route(
            "/api/v1/admin/reports/{id}",
            axum::routing::put(reports::update),
        )
        .route(
            "/api/v1/admin/reports/{id}/photo",
            get(reports::admin_photo),
        )
        .route(
            "/api/v1/admin/reports/{id}/photos/{position}",
            get(reports::admin_gallery_photo),
        )
        .route(
            "/api/v1/admin/reports/{id}/resolution",
            axum::routing::put(reports::update_resolution).layer(DefaultBodyLimit::max(
                crate::reports::REPORT_UPLOAD_BYTES + 64 * 1024,
            )),
        )
        .route(
            "/api/v1/admin/reports/{id}/resolution-photos/{photo_id}",
            get(reports::admin_resolution_photo),
        )
        .route(
            "/api/v1/admin/report-settings",
            get(reports::settings).put(reports::update_settings),
        )
        .route("/api/v1/admin/subscribers", get(subscriber_admin::list))
        .route(
            "/api/v1/admin/subscribers/{id}/export",
            get(subscriber_admin::export),
        )
        .route(
            "/api/v1/admin/subscribers/{id}/withdraw",
            post(subscriber_admin::withdraw),
        )
        .route(
            "/api/v1/admin/subscribers/{id}",
            axum::routing::delete(subscriber_admin::erase),
        )
        .route("/api/v1/search", get(search::endpoint))
        .route("/api/v1/ready", get(ready))
        .route("/api/v1/admin/mail", get(management::mail_history))
        .route(
            "/api/v1/admin/mail-settings",
            get(mail_settings::get).put(mail_settings::update),
        )
        .route(
            "/api/v1/admin/mail-settings/test",
            post(mail_settings::test),
        )
        .route("/api/v1/privacy", get(privacy::public_notice))
        .route(
            "/api/v1/admin/privacy",
            get(privacy::admin_notice)
                .put(privacy::update)
                .layer(DefaultBodyLimit::max(128 * 1024)),
        )
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
            "/api/v1/admin/accounts/{id}/permissions",
            axum::routing::put(accounts::set_permissions),
        )
        .route(
            "/api/v1/admin/password",
            axum::routing::put(accounts::change_password),
        )
        .route(
            "/api/v1/admin/profile",
            axum::routing::put(accounts::update_profile),
        )
        .route("/api/v1/admin/login", post(auth::login))
        .route("/api/v1/admin/password-policy", get(recovery::policy))
        .route("/api/v1/admin/password-recovery", post(recovery::request))
        .route("/api/v1/admin/password-reset", post(recovery::reset))
        .route(
            "/api/v1/admin/session",
            get(auth::session).delete(auth::logout),
        )
        .route("/api/v1/admin/audit", get(audit_list))
        .route("/api/v1/admin/overview", get(management::overview))
        .route(
            "/api/v1/admin/attachment-index/status",
            get(attachment_index::status),
        )
        .route(
            "/api/v1/admin/cez-outages/status",
            get(power_outages::status),
        )
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
            "/api/v1/admin/navigation",
            get(navigation::list).post(navigation::create),
        )
        .route(
            "/api/v1/admin/navigation/{id}",
            axum::routing::put(navigation::update).delete(navigation::remove),
        )
        .route(
            "/api/v1/admin/categories",
            get(navigation::categories).post(navigation::create_category),
        )
        .route(
            "/api/v1/admin/categories/{id}",
            axum::routing::put(navigation::update_category).delete(navigation::remove_category),
        )
        .route(
            "/api/v1/admin/legacy/notices/{id}",
            get(management::legacy_notice),
        )
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
        .route("/api/v1/admin/import-revisions", get(registry::revisions))
        .route(
            "/api/v1/admin/import-revisions/{id}/decision",
            post(registry::decide),
        )
        .route(
            "/api/v1/admin/import-revisions/{id}/files/{position}",
            get(registry::revision_file),
        )
        .route(
            "/api/v1/admin/documents/{id}/preview",
            get(registry::preview),
        )
        .route(
            "/api/v1/admin/documents/{id}/postings",
            post(registry::post),
        )
        .route(
            "/api/v1/admin/documents/{id}/relations",
            post(registry::link),
        )
        .route(
            "/api/v1/admin/documents/{id}/event-corrections",
            post(registry::evidence_correction),
        )
        .route(
            "/api/v1/admin/documents/{id}/corrections",
            post(registry::correction),
        )
        .route(
            "/api/v1/admin/documents/{id}/visibility",
            post(registry::visibility),
        )
        .route("/api/v1/admin/documents/{id}/topics", post(registry::topic))
        .route(
            "/api/v1/admin/documents/{id}/history",
            get(registry::history),
        )
        .route(
            "/api/v1/admin/postings/{id}/withdraw",
            post(registry::withdraw),
        )
        .route("/api/v1/admin/postings/{id}/plan", post(registry::plan))
        .route("/api/v1/documents", get(documents::public_list))
        .route("/api/v1/documents/{id}", get(documents::detail))
        .route(
            "/api/v1/attachments/{id}/text",
            get(attachment_text::preview),
        )
        .route(
            "/api/v1/attachments/{id}/text/download",
            get(attachment_text::download),
        )
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
        .route(
            "/api/v1/pages/{slug}/markdown",
            get(agent_docs::page_markdown),
        )
        .route("/api/v1/admin/pages/{id}/revisions", get(pages::revisions))
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
        .route(
            "/odber/nastaveni",
            get(subscription_preferences::page).post(subscription_preferences::update),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(axum::middleware::from_fn(auth::response_headers))
        .layer(axum::middleware::from_fn_with_state(
            config,
            privacy::response_headers,
        ))
        .with_state(state)
        .merge(mcp_routes)
        .merge(discovery_routes)
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
