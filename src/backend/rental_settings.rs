//! Global rental availability, enforced independently of public navigation.
use super::{Backend, Error, Result, audit, auth, bad, conflict};
use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct Settings {
    pub enabled: bool,
    pub version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    enabled: bool,
    expected_version: i64,
}
pub fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Rezervační systém je vypnutý.",
    )
}
pub async fn enabled(pool: &PgPool) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT enabled FROM rental_settings WHERE id=1")
            .fetch_optional(pool)
            .await?
            .unwrap_or(false),
    )
}
/// Hold until the operation has finished. Writers acquire the global write lock first.
pub(super) async fn enabled_locked(conn: &mut PgConnection) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT enabled FROM rental_settings WHERE id=1 FOR SHARE")
            .fetch_optional(conn)
            .await?
            .unwrap_or(false),
    )
}
pub(super) async fn require_enabled(conn: &mut PgConnection) -> Result<()> {
    if !enabled_locked(conn).await? {
        return Err(unavailable());
    }
    Ok(())
}
/// Reject disabled requests before extractors can read JSON or multipart bodies.
/// The business operation also locks and rechecks the current setting.
pub async fn gate(State(s): State<Backend>, request: Request, next: Next) -> Response {
    let mut response = match enabled(&s.pool).await {
        Ok(true) => next.run(request).await,
        Ok(false) => unavailable().into_response(),
        Err(error) => error.into_response(),
    };
    auth::set_no_store(response.headers_mut());
    response
}
pub async fn read(State(s): State<Backend>, _: auth::Admin) -> Result<Json<Settings>> {
    Ok(Json(
        sqlx::query_as("SELECT enabled,version FROM rental_settings WHERE id=1")
            .fetch_one(&s.pool)
            .await?,
    ))
}
fn lock_error(error: sqlx::Error) -> Error {
    if error
        .as_database_error()
        .is_some_and(|db| db.code().as_deref() == Some("55P03"))
    {
        conflict("Probíhá zpracování rezervace. Zkuste změnu znovu.")
    } else {
        error.into()
    }
}
pub async fn update(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<Input>,
) -> Result<Json<Settings>> {
    if input.expected_version < 1 {
        return Err(bad("Neplatná verze nastavení."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await.map_err(lock_error)?;
    super::rentals::require_rental_write_access(&mut tx, &admin).await?;
    let current: Settings =
        sqlx::query_as("SELECT enabled,version FROM rental_settings WHERE id=1 FOR UPDATE")
            .fetch_one(&mut *tx)
            .await
            .map_err(lock_error)?;
    if current.version != input.expected_version {
        return Err(conflict(
            "Nastavení mezitím změnil jiný správce. Obnovte stránku.",
        ));
    }
    if current.enabled == input.enabled {
        tx.commit().await?;
        return Ok(Json(current));
    }
    let version = current
        .version
        .checked_add(1)
        .ok_or_else(|| bad("Verzi nastavení nelze zvýšit."))?;
    sqlx::query("UPDATE rental_settings SET enabled=$1,version=$2 WHERE id=1")
        .bind(input.enabled)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        Some(admin.id),
        if input.enabled { "enabled" } else { "disabled" },
        "rental_settings",
        1,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Settings {
        enabled: input.enabled,
        version,
    }))
}
