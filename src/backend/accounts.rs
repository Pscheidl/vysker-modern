//! Account lifecycle. Credential and access changes also require reauthentication.
use super::{Backend, Result, audit, auth, conflict, missing};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Serialize, sqlx::FromRow)]
pub struct Account {
    id: i64,
    email: String,
    active: bool,
    sessions: i64,
}

pub async fn list(State(s): State<Backend>, _: auth::Admin) -> Result<Json<Vec<Account>>> {
    Ok(Json(sqlx::query_as("SELECT a.id,a.email,a.active,(SELECT count(*) FROM sessions WHERE administrator_id=a.id AND expires_at>$1) AS sessions FROM administrators a ORDER BY a.email")
        .bind(OffsetDateTime::now_utc().unix_timestamp()).fetch_all(&s.pool).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub current_password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAccount {
    email: String,
    password: String,
    current_password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordChange {
    current_password: String,
    new_password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountState {
    active: bool,
    current_password: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    display_name: Option<String>,
}

/// A public signature default belongs to the authenticated account only.
pub async fn update_profile(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<Profile>,
) -> Result<Json<Profile>> {
    let display_name = input
        .display_name
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if display_name.as_ref().is_some_and(|name| {
        name.chars().count() > crate::reports::REPORT_NAME_LIMIT
            || name.chars().any(char::is_control)
    }) {
        return Err(super::bad(
            "Jméno může mít nejvýše 120 znaků a nesmí obsahovat řídicí znaky.",
        ));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let changed = sqlx::query("UPDATE administrators SET display_name=$1 WHERE id=$2 AND active=TRUE AND EXISTS(SELECT 1 FROM sessions WHERE administrator_id=$2 AND token_hash=$3 AND expires_at>$4)")
        .bind(&display_name)
        .bind(admin.id)
        .bind(&admin.token_hash)
        .bind(OffsetDateTime::now_utc().unix_timestamp())
        .execute(&mut *tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(auth::denied());
    }
    audit(
        &mut tx,
        Some(admin.id),
        "profile_updated",
        "administrator",
        admin.id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Profile { display_name }))
}

// Expensive Argon2 work happens before taking the PostgreSQL advisory write lock.
pub(super) async fn reauthenticate(
    s: &Backend,
    admin: &auth::Admin,
    password: String,
) -> Result<String> {
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

pub(super) async fn current(
    conn: &mut sqlx::PgConnection,
    admin: &auth::Admin,
    password_hash: &str,
) -> Result<()> {
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators a JOIN sessions s ON a.id=s.administrator_id WHERE a.id=$1 AND a.active=TRUE AND a.password_hash=$2 AND s.token_hash=$3 AND s.expires_at>$4)")
        .bind(admin.id).bind(password_hash).bind(&admin.token_hash)
        .bind(OffsetDateTime::now_utc().unix_timestamp()).fetch_one(conn).await?;
    if valid { Ok(()) } else { Err(auth::denied()) }
}

pub async fn create(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<NewAccount>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let verified = reauthenticate(&s, &admin, input.current_password).await?;
    let email = auth::email(&input.email)?;
    let password_hash =
        auth::password_hash(input.password, s.config.minimum_password_length).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    current(&mut tx, &admin, &verified).await?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO administrators(email,password_hash) VALUES ($1,$2) RETURNING id",
    )
    .bind(email)
    .bind(password_hash)
    .fetch_one(&mut *tx)
    .await?;
    audit(
        &mut tx,
        Some(admin.id),
        "created",
        "administrator",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id":id}))))
}

pub async fn change_password(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<PasswordChange>,
) -> Result<StatusCode> {
    let verified = reauthenticate(&s, &admin, input.current_password).await?;
    let replacement =
        auth::password_hash(input.new_password, s.config.minimum_password_length).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    current(&mut tx, &admin, &verified).await?;
    set_password(
        &mut tx,
        admin.id,
        replacement,
        Some(admin.id),
        "password_changed",
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn set_password(
    conn: &mut sqlx::PgConnection,
    id: i64,
    password_hash: String,
    actor: Option<i64>,
    operation: &str,
) -> Result<()> {
    sqlx::query("UPDATE administrators SET password_hash=$1 WHERE id=$2")
        .bind(password_hash)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE administrator_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    super::recovery::invalidate(conn, id).await?;
    audit(
        conn,
        actor,
        operation,
        "administrator",
        id,
        OffsetDateTime::now_utc(),
    )
    .await
}

/// Operator recovery requires access to the server. It never reactivates an account.
pub async fn reset_password(s: &Backend, email: &str, password: String) -> Result<i64> {
    let email = auth::email(email)?;
    let replacement = auth::password_hash(password, s.config.minimum_password_length).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM administrators WHERE email=$1")
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    set_password(&mut tx, id, replacement, None, "password_reset_by_operator").await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn set_active(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<AccountState>,
) -> Result<StatusCode> {
    let verified = reauthenticate(&s, &admin, input.current_password).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    current(&mut tx, &admin, &verified).await?;
    let active: bool = sqlx::query_scalar("SELECT active FROM administrators WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    if active && !input.active {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM administrators WHERE active=TRUE")
                .fetch_one(&mut *tx)
                .await?;
        if count <= 1 {
            return Err(conflict("Posledního aktivního správce nelze deaktivovat."));
        }
    }
    if active != input.active {
        super::recovery::invalidate(&mut tx, id).await?;
        sqlx::query("UPDATE administrators SET active=$1 WHERE id=$2")
            .bind(input.active)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE administrator_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        audit(
            &mut tx,
            Some(admin.id),
            if input.active {
                "activated"
            } else {
                "deactivated"
            },
            "administrator",
            id,
            OffsetDateTime::now_utc(),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn revoke_sessions(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<Credentials>,
) -> Result<StatusCode> {
    let verified = reauthenticate(&s, &admin, input.current_password).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    current(&mut tx, &admin, &verified).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators WHERE id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(missing());
    }
    sqlx::query("DELETE FROM sessions WHERE administrator_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        Some(admin.id),
        "sessions_revoked",
        "administrator",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
