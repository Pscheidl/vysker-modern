use super::{
    Backend, MAX_FILE_BYTES, Pagination, Result, audit,
    auth::{self, Admin},
    bad, conflict, mail, missing, text, timestamp,
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
    Ok(sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE document_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(&s.pool).await?)
}
pub async fn public_notice_files(
    pool: &sqlx::PgPool,
    id: i64,
    now: OffsetDateTime,
) -> Result<Vec<Attachment>> {
    let visible: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM documents d WHERE id=$1 AND document_is_public(d,$2))",
    )
    .bind(id)
    .bind(super::today(now))
    .fetch_one(pool)
    .await?;
    if !visible {
        return Err(missing());
    }
    Ok(sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE document_id=$1 AND is_public AND removed_at IS NULL AND data IS NOT NULL AND EXISTS(SELECT 1 FROM documents d WHERE d.id=$1 AND document_files_public(d,(now() AT TIME ZONE 'Europe/Prague')::date)) ORDER BY sort_order,id").bind(id).fetch_all(pool).await?)
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
    pub document_date: Option<time::Date>,
    pub category_id: Option<i64>,
    pub reference_number: Option<String>,
    pub issuer: Option<String>,
    pub lock_state: String,
    pub public_access: bool,
    pub access_blocked: bool,
    pub public_until: Option<time::Date>,
    pub board_status: Option<String>,
    pub published_on: Option<time::Date>,
    pub withdraw_on: Option<time::Date>,
    #[sqlx(default)]
    pub board_enabled: bool,
    #[sqlx(default)]
    pub duration_days: Option<i64>,
    pub retain_attachments: Option<bool>,
    #[serde(skip_serializing)]
    pub posting_after_action: Option<String>,
    pub content_version: i64,
    pub metadata_restricted: bool,
    pub public_title: Option<String>,
    pub attachments_public: bool,
    #[serde(skip_serializing)]
    pub posting_public_until: Option<time::Date>,
    #[sqlx(default)]
    pub is_public_now: bool,
    #[sqlx(default)]
    pub files_public_now: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub reference_number: Option<String>,
    pub issuer: Option<String>,
    pub document_date: Option<time::Date>,
    pub expected_version: Option<i64>,
    pub category_id: Option<i64>,
    pub board_enabled: Option<bool>,
    pub published_on: Option<time::Date>,
    pub duration_days: Option<i64>,
    pub retain_attachments: Option<bool>,
}
impl DocumentInput {
    fn posting(&self) -> Result<Option<(time::Date, time::Date, bool)>> {
        if self.board_enabled != Some(true) {
            return Ok(None);
        }
        let start = self
            .published_on
            .ok_or_else(|| bad("Vyplňte datum vyvěšení."))?;
        let end = super::registry::posting_end(start, self.duration_days, None)?;
        let retain = self
            .retain_attachments
            .ok_or_else(|| bad("Vyberte dostupnost příloh po sejmutí."))?;
        Ok(Some((start, end, retain)))
    }
    async fn save_board(&self, conn: &mut sqlx::PgConnection, id: i64) -> Result<()> {
        if let Some((start, end, retain)) = self.posting()? {
            sqlx::query("UPDATE documents SET board_status='draft',published_on=$2,withdraw_on=$3,retain_attachments=$4,posting_after_action=CASE WHEN $4 THEN 'retain' ELSE 'hide' END WHERE id=$1")
                .bind(id).bind(start).bind(end).bind(retain).execute(&mut *conn).await?;
        } else if self.board_enabled == Some(false) {
            sqlx::query("UPDATE documents SET board_status=NULL,published_on=NULL,withdraw_on=NULL,posting_after_action=NULL WHERE id=$1").bind(id).execute(&mut *conn).await?;
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        text(&self.title, 300)?;
        if self.description.len() > 50_000
            || self
                .reference_number
                .as_ref()
                .is_some_and(|v| v.len() > 200)
            || self.issuer.as_ref().is_some_and(|v| v.len() > 300)
        {
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
    pub postings: Vec<serde_json::Value>,
    pub relations: Vec<serde_json::Value>,
    pub origin: Option<serde_json::Value>,
}
pub async fn create(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<DocumentInput>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    input.validate()?;
    input.posting()?;
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO documents(title,description,created_at,reference_number,issuer,document_date,category_id) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id",
    )
    .bind(input.title.trim())
    .bind(&input.description)
    .bind(timestamp(now)).bind(&input.reference_number).bind(&input.issuer).bind(input.document_date).bind(input.category_id)
    .fetch_one(&mut *tx)
    .await?;
    input.save_board(&mut tx, id).await?;
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
    input.posting()?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let count =
        sqlx::query("UPDATE documents SET title=$1,description=$2,reference_number=$4,issuer=coalesce($5,issuer),document_date=$6,category_id=coalesce($8,category_id) WHERE id=$3 AND status='draft' AND lock_state='draft' AND board_status IS DISTINCT FROM 'scheduled' AND ($7::bigint IS NULL OR content_version=$7)")
            .bind(input.title.trim())
            .bind(&input.description)
            .bind(id).bind(&input.reference_number).bind(&input.issuer).bind(input.document_date).bind(input.expected_version).bind(input.category_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if count == 0 {
        return Err(conflict("Upravovat lze pouze koncept dokumentu."));
    }
    input.save_board(&mut tx, id).await?;
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
    sqlx::query_scalar::<_, i64>("UPDATE documents SET status='published',public_access=TRUE,published_at=coalesce(published_at,$1) WHERE id=$2 AND NOT public_access AND NOT access_blocked AND lock_state='draft' AND board_status IS NULL RETURNING id")
        .bind(timestamp(now)).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||conflict("Zveřejnit lze pouze koncept dokumentu."))?;
    audit(&mut tx, Some(admin.id), "published", "document", id, now).await?;
    mail::enqueue_publication(&mut tx, mail::PublicationTarget::Document(id), now).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn archive(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<super::registry::Reason>,
) -> Result<StatusCode> {
    text(&input.reason, 2000)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let changed=sqlx::query("UPDATE documents SET status='archived' WHERE id=$1 AND board_status IS NULL AND status='published'").bind(id).execute(&mut *tx).await?.rows_affected();
    if changed == 0 {
        return Err(conflict(
            "Do archivu lze přesunout zveřejněný dokument. Vyvěšený dokument nejprve sejměte z desky.",
        ));
    }
    super::registry::event(
        &mut tx,
        id,
        Some(admin.id),
        "catalog_archived",
        &input.reason,
        serde_json::json!({}),
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
        sqlx::query_as::<_,Document>("SELECT id,title,left(description,400) AS description,status,created_at,published_at,source_published_on,document_date,category_id,reference_number,issuer,lock_state,public_access,access_blocked,public_until,board_status,published_on,withdraw_on,retain_attachments,posting_after_action,content_version,metadata_restricted,public_title,attachments_public,posting_public_until FROM documents ORDER BY catalog_date DESC NULLS LAST,id DESC LIMIT $1 OFFSET $2")
            .bind(limit)
            .bind(offset)
            .fetch_all(&s.pool)
            .await?.into_iter().map(Document::with_board_settings).collect(),
    ))
}
pub async fn public_list(
    State(s): State<Backend>,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<Document>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(
        sqlx::query_as::<_,Document>(
            "SELECT id,title,left(description,400) AS description,status,created_at,published_at,source_published_on,document_date,category_id,reference_number,issuer,lock_state,public_access,access_blocked,public_until,board_status,published_on,withdraw_on,retain_attachments,posting_after_action,content_version,metadata_restricted,public_title,attachments_public,posting_public_until FROM documents WHERE document_is_public(documents,(now() AT TIME ZONE 'Europe/Prague')::date) ORDER BY catalog_date DESC NULLS LAST,id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&s.pool)
        .await?.into_iter().map(Document::into_public).collect(),
    ))
}
pub async fn detail(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Json<DocumentDetail>> {
    let record: Document = sqlx::query_as("SELECT * FROM documents WHERE id=$1 AND document_is_public(documents,(now() AT TIME ZONE 'Europe/Prague')::date)")
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    let files=sqlx::query_as("SELECT id,name,content_type,size_bytes,removed_at,(data IS NOT NULL AND removed_at IS NULL) AS available FROM attachments WHERE document_id=$1 AND is_public AND removed_at IS NULL AND data IS NOT NULL AND EXISTS(SELECT 1 FROM documents d WHERE d.id=$1 AND document_files_public(d,(now() AT TIME ZONE 'Europe/Prague')::date)) ORDER BY sort_order,id").bind(id).fetch_all(&s.pool).await?;
    Ok(Json(DocumentDetail {
        record: record.into_public(),
        attachments: files,
        postings: super::registry::postings(&s.pool, id, true).await?,
        relations: public_relations(&s.pool, id).await?,
        origin: None,
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
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents WHERE id=$1 AND status='draft' AND lock_state='draft' AND board_status IS DISTINCT FROM 'scheduled')")
            .bind(parent)
            .fetch_one(&mut *tx)
            .await?
    } else {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents WHERE id=$1 AND status='draft' AND lock_state='draft' AND board_status IS DISTINCT FROM 'scheduled')")
            .bind(parent)
            .fetch_one(&mut *tx)
            .await?
    };
    if !editable {
        return Err(conflict("Přílohy lze nahrát pouze ke konceptu."));
    }
    let id=sqlx::query_scalar::<_, i64>("INSERT INTO attachments(notice_id,document_id,name,content_type,size_bytes,data,sha256) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
        .bind(if notice {Some(parent)}else{None}).bind(Some(parent)).bind(name).bind(mime).bind(bytes.len() as i64).bind(bytes.as_ref()).bind(auth::hash(&bytes)).fetch_one(&mut *tx).await?;
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
    // Visibility and bytes are read in one statement, including every historical URL.
    let (name,mime,bytes):(String,String,Vec<u8>)=sqlx::query_as("SELECT a.name,a.content_type,a.data FROM attachments a JOIN documents d ON d.id=a.document_id WHERE a.id=$1 AND a.is_public AND a.data IS NOT NULL AND a.removed_at IS NULL AND document_files_public(d,$2)").bind(id).bind(super::today(OffsetDateTime::now_utc())).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, "no-store, private".parse().unwrap());
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

pub async fn public_relations(pool: &sqlx::PgPool, id: i64) -> Result<Vec<serde_json::Value>> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',d.id,'title',CASE WHEN d.metadata_restricted THEN d.public_title ELSE d.title END,'kind',r.kind,'direction',CASE WHEN r.original_id=$1 THEN 'outgoing' ELSE 'incoming' END) FROM document_relations r JOIN documents d ON d.id=CASE WHEN r.original_id=$1 THEN r.related_id ELSE r.original_id END WHERE (r.original_id=$1 OR r.related_id=$1) AND document_is_public(d,(now() AT TIME ZONE 'Europe/Prague')::date) ORDER BY r.id LIMIT 100").bind(id).fetch_all(pool).await?)
}

impl Document {
    pub fn with_board_settings(mut self) -> Self {
        let day = super::today(OffsetDateTime::now_utc());
        self.is_public_now = self.public_access
            && !self.access_blocked
            && self.public_until.is_none_or(|until| until > day);
        self.files_public_now = self.is_public_now
            && self.attachments_public
            && (!(self.metadata_restricted
                || matches!(self.board_status.as_deref(), Some("archived" | "withdrawn")))
                || self.posting_public_until.is_none_or(|until| until > day));
        self.board_enabled = self.board_status.is_some();
        self.duration_days = self
            .published_on
            .zip(self.withdraw_on)
            .map(|(start, end)| (end - start).whole_days() - 1)
            .filter(|days| *days > 0);
        self.retain_attachments = self
            .posting_after_action
            .as_deref()
            .map(|action| action == "retain");
        self
    }
    pub fn into_public(self) -> Self {
        let mut record = self.with_board_settings();
        if record.metadata_restricted {
            record.title = record
                .public_title
                .clone()
                .unwrap_or_else(|| format!("Záznam úřední desky #{}", record.id));
            record.description.clear();
            record.reference_number = None;
            record.issuer = None;
        }
        record
    }
}
