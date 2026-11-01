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
    is_primary: bool,
    can_view_identity: bool,
    can_manage_permissions: bool,
    can_create_accounts: bool,
    can_manage_mail_settings: bool,
}

pub async fn list(State(s): State<Backend>, _: auth::Admin) -> Result<Json<Vec<Account>>> {
    Ok(Json(sqlx::query_as("SELECT a.id,a.email,a.active,a.is_primary,a.can_view_identity,a.can_manage_permissions,a.can_create_accounts,a.can_manage_mail_settings,(SELECT count(*) FROM sessions WHERE administrator_id=a.id AND expires_at>$1) AS sessions FROM administrators a ORDER BY a.email")
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
    #[serde(default)]
    can_view_identity: bool,
    #[serde(default)]
    can_manage_permissions: bool,
    #[serde(default)]
    can_create_accounts: bool,
    #[serde(default)]
    can_manage_mail_settings: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionChange {
    current_password: String,
    can_view_identity: bool,
    can_manage_permissions: bool,
    can_create_accounts: bool,
    can_manage_mail_settings: bool,
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

pub(super) async fn require_permission(
    conn: &mut sqlx::PgConnection,
    id: i64,
    permission: &str,
) -> Result<()> {
    let query = match permission {
        "manage" => "SELECT can_manage_permissions FROM administrators WHERE id=$1 AND active=TRUE",
        "identity" => "SELECT can_view_identity FROM administrators WHERE id=$1 AND active=TRUE",
        "create" => "SELECT can_create_accounts FROM administrators WHERE id=$1 AND active=TRUE",
        "mail" => "SELECT can_manage_mail_settings FROM administrators WHERE id=$1 AND active=TRUE",
        _ => {
            return Err(super::Error(
                StatusCode::FORBIDDEN,
                "K této akci nemáte oprávnění.",
            ));
        }
    };
    let allowed: bool = sqlx::query_scalar(query)
        .bind(id)
        .fetch_optional(conn)
        .await?
        .unwrap_or(false);
    if allowed {
        Ok(())
    } else {
        Err(super::Error(
            StatusCode::FORBIDDEN,
            "K této akci nemáte oprávnění.",
        ))
    }
}

type Permissions = (bool, bool, bool, bool);

fn validate_permissions(value: Permissions) -> Result<()> {
    if value.3 && !(value.0 && value.1 && value.2) {
        return Err(super::bad(
            "Správu odesílacího účtu lze přidělit pouze správci se všemi ostatními oprávněními.",
        ));
    }
    Ok(())
}
async fn permissions(conn: &mut sqlx::PgConnection, id: i64) -> Result<Permissions> {
    sqlx::query_as("SELECT can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings FROM administrators WHERE id=$1 AND active=TRUE")
        .bind(id).fetch_optional(conn).await?.ok_or_else(auth::denied)
}

async fn ensure_another_active_full(conn: &mut sqlx::PgConnection, excluding: i64) -> Result<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators WHERE id<>$1 AND active AND can_view_identity AND can_manage_permissions AND can_create_accounts AND can_manage_mail_settings)")
        .bind(excluding).fetch_one(conn).await?;
    if !exists {
        return Err(conflict(
            "Musí zůstat alespoň jeden aktivní správce se všemi oprávněními.",
        ));
    }
    Ok(())
}

fn require_owned_changes(actor: Permissions, old: Permissions, new: Permissions) -> Result<()> {
    if (old.0 != new.0 && !actor.0)
        || (old.1 != new.1 && !actor.1)
        || (old.2 != new.2 && !actor.2)
        || (old.3 != new.3 && !actor.3)
    {
        return Err(super::Error(
            StatusCode::FORBIDDEN,
            "Správce může měnit pouze oprávnění, která sám má.",
        ));
    }
    Ok(())
}

async fn require_target_not_stronger(
    conn: &mut sqlx::PgConnection,
    actor_id: i64,
    target_id: i64,
) -> Result<()> {
    if actor_id == target_id {
        return Ok(());
    }
    let actor = permissions(conn, actor_id).await?;
    let target: Permissions = sqlx::query_as("SELECT can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings FROM administrators WHERE id=$1")
        .bind(target_id).fetch_optional(conn).await?.ok_or_else(missing)?;
    if (target.0 && !actor.0)
        || (target.1 && !actor.1)
        || (target.2 && !actor.2)
        || (target.3 && !actor.3)
    {
        return Err(super::Error(
            StatusCode::FORBIDDEN,
            "Nemůžete spravovat účet se silnějšími oprávněními.",
        ));
    }
    Ok(())
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
    require_permission(&mut tx, admin.id, "create").await?;
    let actor = permissions(&mut tx, admin.id).await?;
    validate_permissions((
        input.can_view_identity,
        input.can_manage_permissions,
        input.can_create_accounts,
        input.can_manage_mail_settings,
    ))?;
    require_owned_changes(
        actor,
        (false, false, false, false),
        (
            input.can_view_identity,
            input.can_manage_permissions,
            input.can_create_accounts,
            input.can_manage_mail_settings,
        ),
    )?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO administrators(email,password_hash,can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id",
    )
    .bind(email)
    .bind(password_hash)
    .bind(input.can_view_identity)
    .bind(input.can_manage_permissions)
    .bind(input.can_create_accounts)
    .bind(input.can_manage_mail_settings)
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

pub async fn set_permissions(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<PermissionChange>,
) -> Result<StatusCode> {
    let verified = reauthenticate(&s, &admin, input.current_password).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    current(&mut tx, &admin, &verified).await?;
    require_permission(&mut tx, admin.id, "manage").await?;
    let (primary, active, old_view, old_manage, old_create, old_mail): (bool, bool, bool, bool, bool, bool) = sqlx::query_as("SELECT is_primary,active,can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings FROM administrators WHERE id=$1")
        .bind(id).fetch_optional(&mut *tx).await?.ok_or_else(missing)?;
    if primary
        && (!input.can_view_identity
            || !input.can_manage_permissions
            || !input.can_create_accounts
            || !input.can_manage_mail_settings)
    {
        return Err(conflict("Prvnímu správci nelze odebrat oprávnění."));
    }
    let old = (old_view, old_manage, old_create, old_mail);
    let new = (
        input.can_view_identity,
        input.can_manage_permissions,
        input.can_create_accounts,
        input.can_manage_mail_settings,
    );
    validate_permissions(new)?;
    let actor = permissions(&mut tx, admin.id).await?;
    require_owned_changes(actor, old, new)?;
    if active && old == (true, true, true, true) && new != old {
        ensure_another_active_full(&mut tx, id).await?;
    }
    if old != new {
        sqlx::query(
            "UPDATE administrators SET can_view_identity=$1,can_manage_permissions=$2,can_create_accounts=$3,can_manage_mail_settings=$4 WHERE id=$5",
        )
        .bind(input.can_view_identity)
        .bind(input.can_manage_permissions)
        .bind(input.can_create_accounts)
        .bind(input.can_manage_mail_settings)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        audit(
            &mut tx,
            Some(admin.id),
            "permissions_updated",
            "administrator",
            id,
            OffsetDateTime::now_utc(),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
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
    require_target_not_stronger(&mut tx, admin.id, id).await?;
    if active && !input.active {
        let target: Permissions = sqlx::query_as("SELECT can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings FROM administrators WHERE id=$1")
            .bind(id).fetch_one(&mut *tx).await?;
        if target == (true, true, true, true) {
            ensure_another_active_full(&mut tx, id).await?;
        }
    }
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
    require_target_not_stronger(&mut tx, admin.id, id).await?;
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
