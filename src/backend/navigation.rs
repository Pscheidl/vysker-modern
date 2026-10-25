use super::{Backend, Result, audit, auth::Admin, bad, conflict, missing, text};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemInput {
    label: String,
    path: String,
    sort_order: i64,
    visible: bool,
    parent_id: Option<i64>,
    expected_version: Option<i64>,
}
#[derive(Serialize, sqlx::FromRow)]
pub struct Item {
    id: i64,
    label: String,
    path: String,
    sort_order: i64,
    visible: bool,
    parent_id: Option<i64>,
    version: i64,
}
pub async fn list(State(s): State<Backend>, _: Admin) -> Result<Json<Vec<Item>>> {
    Ok(Json(
        sqlx::query_as("SELECT * FROM navigation_items ORDER BY sort_order,id")
            .fetch_all(&s.pool)
            .await?,
    ))
}
async fn validate(
    conn: &mut sqlx::PgConnection,
    input: &ItemInput,
    item_id: Option<i64>,
) -> Result<()> {
    text(&input.label, 40)?;
    if !(0..=9999).contains(&input.sort_order) {
        return Err(bad("Pořadí musí být mezi 0 a 9999."));
    }
    let valid = matches!(
        input.path.as_str(),
        "/" | "/uredni-deska"
            | "/dokumenty"
            | "/obec"
            | "/kontakt"
            | "/kalendar"
            | "/hlaseni-zavad"
            | "/hlaseni-zavad/nove"
            | "/odber"
            | "/stranky"
            | "/pristupnost"
            | "/povinne-informace"
            | "/ochrana-udaju"
    );
    if !valid {
        let Some(slug) = input.path.strip_prefix("/stranky/") else {
            return Err(bad("Vyberte existující stránku tohoto webu."));
        };
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pages WHERE slug=$1 AND (published=TRUE OR $2=FALSE))",
        )
        .bind(slug)
        .bind(input.visible)
        .fetch_one(&mut *conn)
        .await?;
        if !exists {
            return Err(bad("Stránka neexistuje nebo není zveřejněná."));
        }
    }
    if let Some(parent_id) = input.parent_id {
        if Some(parent_id) == item_id {
            return Err(bad("Položka nemůže být podřazená sama sobě."));
        }
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_id FROM navigation_items WHERE id=$1")
                .bind(parent_id)
                .fetch_optional(&mut *conn)
                .await?;
        match parent {
            None => {
                return Err(bad(
                    "Nadřazená položka už neexistuje. Načtěte navigaci znovu.",
                ));
            }
            Some((Some(_),)) => {
                return Err(bad(
                    "Podmenu může mít jen jednu úroveň. Vyberte hlavní položku navigace.",
                ));
            }
            Some((None,)) => {}
        }
        if let Some(id) = item_id {
            let has_children: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM navigation_items WHERE parent_id=$1)",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
            if has_children {
                return Err(bad(
                    "Tato položka má vlastní podmenu. Nejprve přesuňte nebo odstraňte její podřízené položky.",
                ));
            }
        }
    } else {
        let root_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM navigation_items WHERE parent_id IS NULL AND ($1::BIGINT IS NULL OR id<>$1)",
        )
        .bind(item_id)
        .fetch_one(&mut *conn)
        .await?;
        if root_count >= 20 {
            return Err(conflict(
                "Navigace může mít nejvýše 20 hlavních položek. Další odkazy zařaďte do podmenu.",
            ));
        }
    }
    Ok(())
}
async fn check_version(
    conn: &mut sqlx::PgConnection,
    id: i64,
    expected_version: Option<i64>,
) -> Result<()> {
    let current: Option<i64> =
        sqlx::query_scalar("SELECT version FROM navigation_items WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?;
    if current.is_none() || current != expected_version {
        return Err(conflict(
            "Položku mezitím někdo změnil. Načtěte aktuální verzi.",
        ));
    }
    Ok(())
}
pub async fn create(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<ItemInput>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    validate(&mut tx, &input, None).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM navigation_items")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 60 {
        return Err(conflict(
            "Navigace může mít nejvýše 60 položek včetně podmenu.",
        ));
    }
    let id:i64=sqlx::query_scalar("INSERT INTO navigation_items(label,path,sort_order,visible,parent_id) VALUES ($1,$2,$3,$4,$5) RETURNING id").bind(input.label.trim()).bind(input.path).bind(input.sort_order).bind(input.visible).bind(input.parent_id).fetch_one(&mut *tx).await?;
    audit(
        &mut tx,
        Some(admin.id),
        "created",
        "navigation",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id":id}))))
}
pub async fn update(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<ItemInput>,
) -> Result<StatusCode> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    check_version(&mut tx, id, input.expected_version).await?;
    validate(&mut tx, &input, Some(id)).await?;
    let count=sqlx::query("UPDATE navigation_items SET label=$1,path=$2,sort_order=$3,visible=$4,parent_id=$5,version=version+1 WHERE id=$6 AND version=$7").bind(input.label.trim()).bind(input.path).bind(input.sort_order).bind(input.visible).bind(input.parent_id).bind(id).bind(input.expected_version).execute(&mut *tx).await?.rows_affected();
    if count == 0 {
        return Err(conflict(
            "Položku mezitím někdo změnil. Načtěte aktuální verzi.",
        ));
    }
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "navigation",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub expected_version: i64,
}
pub async fn remove(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<Version>,
) -> Result<StatusCode> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    check_version(&mut tx, id, Some(input.expected_version)).await?;
    let has_children: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM navigation_items WHERE parent_id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if has_children {
        return Err(conflict(
            "Položka má vlastní podmenu. Nejprve přesuňte nebo odstraňte její podřízené položky.",
        ));
    }
    let count = sqlx::query("DELETE FROM navigation_items WHERE id=$1 AND version=$2")
        .bind(id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if count == 0 {
        return Err(conflict(
            "Položku mezitím někdo změnil. Načtěte aktuální verzi.",
        ));
    }
    audit(
        &mut tx,
        Some(admin.id),
        "deleted",
        "navigation",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Category {
    id: i64,
    name: String,
    sort_order: i64,
    version: i64,
    usage_count: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryInput {
    name: String,
    sort_order: i64,
    expected_version: Option<i64>,
}
impl CategoryInput {
    fn validate(&self) -> Result<()> {
        text(&self.name, 80)?;
        if !(0..=9999).contains(&self.sort_order) {
            return Err(bad("Pořadí musí být mezi 0 a 9999."));
        }
        Ok(())
    }
}
pub async fn categories(State(s): State<Backend>, _: Admin) -> Result<Json<Vec<Category>>> {
    Ok(Json(sqlx::query_as("SELECT c.*,(SELECT count(*) FROM notices n WHERE n.category_id=c.id) AS usage_count FROM categories c ORDER BY sort_order,id").fetch_all(&s.pool).await?))
}
pub async fn create_category(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<CategoryInput>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    input.validate()?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id: i64 =
        sqlx::query_scalar("INSERT INTO categories(name,sort_order) VALUES ($1,$2) RETURNING id")
            .bind(input.name.trim())
            .bind(input.sort_order)
            .fetch_one(&mut *tx)
            .await?;
    audit(
        &mut tx,
        Some(admin.id),
        "created",
        "category",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id":id}))))
}
pub async fn update_category(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<CategoryInput>,
) -> Result<StatusCode> {
    input.validate()?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let count = sqlx::query(
        "UPDATE categories SET name=$1,sort_order=$2,version=version+1 WHERE id=$3 AND version=$4",
    )
    .bind(input.name.trim())
    .bind(input.sort_order)
    .bind(id)
    .bind(input.expected_version)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if count == 0 {
        return Err(conflict(
            "Kategorii mezitím někdo změnil. Načtěte aktuální verzi.",
        ));
    }
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "category",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn remove_category(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<Version>,
) -> Result<StatusCode> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let used: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notices WHERE category_id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if used {
        return Err(conflict(
            "Používanou kategorii nelze odstranit. Můžete změnit její název nebo pořadí.",
        ));
    }
    let count = sqlx::query("DELETE FROM categories WHERE id=$1 AND version=$2")
        .bind(id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if count == 0 {
        return Err(missing());
    }
    audit(
        &mut tx,
        Some(admin.id),
        "deleted",
        "category",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
