use super::{Backend, Pagination, Result, audit, auth::Admin, bad, missing, text, timestamp};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageInput {
    pub slug: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub published: bool,
    pub expected_version: Option<i64>,
}
impl PageInput {
    fn validate(&self) -> Result<()> {
        text(&self.title, 200)?;
        text(&self.content, 100_000)?;
        if self.slug.is_empty()
            || self.slug.len() > 80
            || self.slug.starts_with('-')
            || self.slug.ends_with('-')
            || !self
                .slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(bad(
                "Slug smí obsahovat malá písmena bez diakritiky, číslice a pomlčky.",
            ));
        }
        Ok(())
    }
}
#[derive(sqlx::FromRow, Serialize)]
pub struct Page {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub content: String,
    pub published: bool,
    pub updated_at: String,
    pub version: i64,
}
pub async fn create(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<PageInput>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    input.validate()?;
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id = sqlx::query_scalar::<_, i64>("INSERT INTO pages(slug,title,content,published,updated_at) VALUES ($1,$2,$3,$4,$5) RETURNING id")
    .bind(&input.slug)
    .bind(input.title.trim())
    .bind(input.content)
    .bind(input.published)
    .bind(timestamp(now))
    .fetch_one(&mut *tx)
    .await?
    ;
    snapshot(&mut tx, id, admin.id).await?;
    audit(&mut tx, Some(admin.id), "created", "page", id, now).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id":id}))))
}
pub async fn update(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<PageInput>,
) -> Result<StatusCode> {
    input.validate()?;
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let old_slug: String = sqlx::query_scalar("SELECT slug FROM pages WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    let version = input
        .expected_version
        .ok_or_else(|| bad("Chybí verze stránky. Načtěte ji znovu."))?;
    if s.config.production {
        let old: Option<String> = sqlx::query_scalar("SELECT slug FROM pages WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if old
            .as_deref()
            .is_some_and(|slug| super::server::REQUIRED_PAGES.contains(&slug))
            && (!input.published || old.as_deref() != Some(input.slug.as_str()))
        {
            return Err(super::conflict(
                "Povinnou produkční stránku nelze skrýt ani přejmenovat. Upravte její obsah.",
            ));
        }
    }
    let count = sqlx::query(
        "UPDATE pages SET slug=$1,title=$2,content=$3,published=$4,updated_at=$5,version=version+1 WHERE id=$6 AND version=$7",
    )
    .bind(&input.slug)
    .bind(input.title.trim())
    .bind(input.content)
    .bind(input.published)
    .bind(timestamp(now))
    .bind(id)
    .bind(version)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if count == 0 {
        return Err(super::conflict(
            "Stránku mezitím změnil jiný správce. Váš text zůstává v editoru, načtěte aktuální verzi v novém okně.",
        ));
    }
    sqlx::query("UPDATE navigation_items SET path=$1,version=version+1 WHERE path=$2 AND path<>$1")
        .bind(format!("/stranky/{}", input.slug))
        .bind(format!("/stranky/{old_slug}"))
        .execute(&mut *tx)
        .await?;
    snapshot(&mut tx, id, admin.id).await?;
    audit(&mut tx, Some(admin.id), "updated", "page", id, now).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn admin_list(
    State(s): State<Backend>,
    _: Admin,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<Page>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(
        sqlx::query_as("SELECT * FROM pages ORDER BY id DESC LIMIT $1 OFFSET $2")
            .bind(limit)
            .bind(offset)
            .fetch_all(&s.pool)
            .await?,
    ))
}
pub async fn detail(State(s): State<Backend>, Path(slug): Path<String>) -> Result<Json<Page>> {
    Ok(Json(
        sqlx::query_as("SELECT * FROM pages WHERE slug=$1 AND published=TRUE")
            .bind(slug)
            .fetch_optional(&s.pool)
            .await?
            .ok_or_else(missing)?,
    ))
}

async fn snapshot(conn: &mut sqlx::PgConnection, id: i64, actor: i64) -> Result<()> {
    sqlx::query("INSERT INTO page_revisions(page_id,version,title,content,slug,published,saved_at,actor_id) SELECT id,version,title,content,slug,published,updated_at,$2 FROM pages WHERE id=$1")
        .bind(id).bind(actor).execute(conn).await?;
    Ok(())
}
#[derive(Serialize, sqlx::FromRow)]
pub struct Revision {
    id: i64,
    version: i64,
    title: String,
    content: String,
    slug: String,
    published: bool,
    saved_at: String,
    actor_id: Option<i64>,
}
pub async fn revisions(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<Revision>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(sqlx::query_as("SELECT * FROM page_revisions WHERE page_id=$1 ORDER BY version DESC LIMIT $2 OFFSET $3").bind(id).bind(limit.min(20)).bind(offset).fetch_all(&s.pool).await?))
}
