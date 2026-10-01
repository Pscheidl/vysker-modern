use super::{
    Backend, MAX_FILE_BYTES, Pagination, Result, audit,
    auth::{self, Admin},
    bad, conflict, mail, missing, text, timestamp, today,
};
use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(sqlx::FromRow, Serialize)]
pub struct Attachment {
    pub id: i64,
    pub name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub removed_at: Option<String>,
    pub available: bool,
}
pub async fn notice_files(s: &Backend, id: i64) -> Result<Vec<Attachment>> {
    Ok(sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE notice_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(&s.pool).await?)
}
pub async fn public_notice_files(
    pool: &sqlx::PgPool,
    id: i64,
    now: OffsetDateTime,
) -> Result<Vec<Attachment>> {
    let record:super::notices::NoticeRecord=sqlx::query_as("SELECT * FROM notices WHERE id=$1 AND status IN ('published','archived','withdrawn') AND (status='archived' OR published_on IS NULL OR published_on<=$2)").bind(id).bind(today(now)).fetch_optional(pool).await?.ok_or_else(missing)?;
    let visible = record.files_public(now);
    let mut files:Vec<Attachment>=sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE notice_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(pool).await?;
    if !visible {
        for file in &mut files {
            file.name = format!("Příloha #{}", file.id);
            file.available = false;
        }
    }
    Ok(files)
}
#[derive(sqlx::FromRow, Serialize)]
pub struct Document {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub status: String,
    pub created_at: String,
    pub published_at: Option<String>,
    pub source_published_on: Option<time::Date>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
}
impl DocumentInput {
    fn validate(&self) -> Result<()> {
        text(&self.title, 300)?;
        if self.description.len() > 50_000 {
            return Err(bad("Popis je příliš dlouhý."));
        }
        Ok(())
    }
}
#[derive(Serialize)]
pub struct DocumentDetail {
    #[serde(flatten)]
    pub record: Document,
    pub attachments: Vec<Attachment>,
}
pub async fn create(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<DocumentInput>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    input.validate()?;
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO documents(title,description,created_at) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(input.title.trim())
    .bind(input.description)
    .bind(timestamp(now))
    .fetch_one(&mut *tx)
    .await?;
    audit(&mut tx, Some(admin.id), "created", "document", id, now).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id":id}))))
}
pub async fn update(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<DocumentInput>,
) -> Result<StatusCode> {
    input.validate()?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let count =
        sqlx::query("UPDATE documents SET title=$1,description=$2 WHERE id=$3 AND status='draft'")
            .bind(input.title.trim())
            .bind(input.description)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if count == 0 {
        return Err(conflict("Upravovat lze pouze koncept dokumentu."));
    }
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "document",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn publish(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let title: String=sqlx::query_scalar("UPDATE documents SET status='published',published_at=$1 WHERE id=$2 AND status='draft' RETURNING title")
        .bind(timestamp(now)).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||conflict("Zveřejnit lze pouze koncept dokumentu."))?;
    audit(&mut tx, Some(admin.id), "published", "document", id, now).await?;
    mail::enqueue_publication(
        &mut tx,
        &s,
        "document",
        id,
        &title,
        &format!("/dokumenty/{id}"),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn archive(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let count =
        sqlx::query("UPDATE documents SET status='archived' WHERE id=$1 AND status='published'")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if count == 0 {
        return Err(conflict("Archivovat lze pouze zveřejněný dokument."));
    }
    audit(
        &mut tx,
        Some(admin.id),
        "archived",
        "document",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn admin_list(
    State(s): State<Backend>,
    _: Admin,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<Document>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(
        sqlx::query_as("SELECT * FROM documents ORDER BY coalesce(source_published_on::text,published_at) DESC NULLS LAST,id DESC LIMIT $1 OFFSET $2")
            .bind(limit)
            .bind(offset)
            .fetch_all(&s.pool)
            .await?,
    ))
}
pub async fn public_list(
    State(s): State<Backend>,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<Document>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(
        sqlx::query_as(
            "SELECT * FROM documents WHERE status='published' ORDER BY coalesce(source_published_on::text,published_at) DESC NULLS LAST,id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&s.pool)
        .await?,
    ))
}
pub async fn detail(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Json<DocumentDetail>> {
    let record = sqlx::query_as("SELECT * FROM documents WHERE id=$1 AND status='published'")
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    let files=sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE document_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(&s.pool).await?;
    Ok(Json(DocumentDetail {
        record,
        attachments: files,
    }))
}

pub async fn upload_notice(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    upload(&s, admin.id, id, true, multipart).await
}
pub async fn upload_document(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    upload(&s, admin.id, id, false, multipart).await
}
async fn upload(
    s: &Backend,
    actor: i64,
    parent: i64,
    notice: bool,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let field = multipart
        .next_field()
        .await
        .map_err(|_| bad("Neplatné nahrání souboru."))?
        .ok_or_else(|| bad("Chybí soubor."))?;
    if field.name() != Some("file") {
        return Err(bad("Očekáváno pole file."));
    }
    let name = field
        .file_name()
        .ok_or_else(|| bad("Chybí název souboru."))?
        .to_owned();
    if name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
        || name.len() > 180
        || name.trim().is_empty()
    {
        return Err(bad("Neplatný název souboru."));
    }
    let bytes = field
        .bytes()
        .await
        .map_err(|_| bad("Soubor nelze přečíst nebo překračuje limit 10 MiB."))?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err(bad("Soubor musí mít 1 bajt až 10 MiB."));
    }
    if multipart
        .next_field()
        .await
        .map_err(|_| bad("Neplatné nahrání souboru."))?
        .is_some()
    {
        return Err(bad("Nahrajte jeden soubor v jednom požadavku."));
    }
    let mime = content_type(&name, &bytes)?;
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let editable: bool = if notice {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notices WHERE id=$1 AND status='draft')")
            .bind(parent)
            .fetch_one(&mut *tx)
            .await?
    } else {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents WHERE id=$1 AND status='draft')")
            .bind(parent)
            .fetch_one(&mut *tx)
            .await?
    };
    if !editable {
        return Err(conflict("Přílohy lze nahrát pouze ke konceptu."));
    }
    let id=sqlx::query_scalar::<_, i64>("INSERT INTO attachments(notice_id,document_id,name,content_type,size_bytes,data,sha256) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
        .bind(if notice {Some(parent)}else{None}).bind(if notice {None}else{Some(parent)}).bind(name).bind(mime).bind(bytes.len() as i64).bind(bytes.as_ref()).bind(auth::hash(&bytes)).fetch_one(&mut *tx).await?;
    audit(&mut tx, Some(actor), "uploaded", "attachment", id, now).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id":id}))))
}
fn content_type(name: &str, bytes: &[u8]) -> Result<&'static str> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" if bytes.starts_with(b"%PDF-") => Ok("application/pdf"),
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Ok("image/png"),
        "jpg" | "jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => Ok("image/jpeg"),
        "txt" if std::str::from_utf8(bytes).is_ok() => Ok("text/plain; charset=utf-8"),
        "csv" if std::str::from_utf8(bytes).is_ok() => Ok("text/csv; charset=utf-8"),
        "docx" | "xlsx" | "odt" | "ods" if bytes.starts_with(b"PK\x03\x04") => {
            Ok("application/octet-stream")
        }
        _ => Err(bad(
            "Podporované soubory: PDF, PNG, JPEG, TXT, CSV, DOCX, XLSX, ODT, ODS. Obsah musí odpovídat formátu.",
        )),
    }
}
pub async fn download(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Response> {
    let now = OffsetDateTime::now_utc();
    let parent:Option<(Option<i64>,Option<String>)>=sqlx::query_as("SELECT f.notice_id,d.status FROM attachments f LEFT JOIN documents d ON d.id=f.document_id WHERE f.id=$1 AND f.data IS NOT NULL AND f.removed_at IS NULL").bind(id).fetch_optional(&s.pool).await?;
    let (notice_id, document_status) = parent.ok_or_else(missing)?;
    if let Some(notice_id) = notice_id {
        let record: super::notices::NoticeRecord =
            sqlx::query_as("SELECT * FROM notices WHERE id=$1")
                .bind(notice_id)
                .fetch_one(&s.pool)
                .await?;
        if !record.files_public(now) {
            return Err(missing());
        }
    } else if document_status.as_deref() != Some("published") {
        return Err(missing());
    }
    let (name,mime,bytes):(String,String,Vec<u8>)=sqlx::query_as("SELECT name,content_type,data FROM attachments WHERE id=$1 AND data IS NOT NULL AND removed_at IS NULL").bind(id).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, mime.parse().map_err(|_| missing())?);
    let encoded = percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC);
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"dokument\"; filename*=UTF-8''{encoded}")
            .parse()
            .map_err(|_| missing())?,
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        "sandbox; default-src 'none'".parse().unwrap(),
    );
    Ok((headers, Body::from(bytes)).into_response())
}
