//! Page-owned images, retained so older page revisions can still be restored.
use super::{Backend, MAX_FILE_BYTES, Result, audit, auth::Admin, bad, missing, timestamp};
use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use image::{ImageFormat, ImageReader, Limits};
use serde::Serialize;
use std::{collections::BTreeSet, io::Cursor};
use time::OffsetDateTime;

#[derive(Serialize, sqlx::FromRow)]
pub struct PageImage {
    id: i64,
    name: String,
    size_bytes: i64,
    width: Option<i32>,
    height: Option<i32>,
    url: String,
    preview_url: String,
}

pub async fn list(
    State(s): State<Backend>,
    _: Admin,
    Path(page): Path<i64>,
) -> Result<Json<Vec<PageImage>>> {
    let content: String = sqlx::query_scalar("SELECT content FROM pages WHERE id=$1")
        .bind(page)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    let mut imported = BTreeSet::new();
    collect_imported(&content, &mut imported);
    // Read bounded batches so old photographs remain available after removal
    // from the current body, without loading the entire revision history at once.
    let mut before = i64::MAX;
    loop {
        let revisions: Vec<(i64, String)> = sqlx::query_as("SELECT version,content FROM page_revisions WHERE page_id=$1 AND version<$2 ORDER BY version DESC LIMIT 20")
            .bind(page).bind(before).fetch_all(&s.pool).await?;
        for (version, content) in &revisions {
            collect_imported(content, &mut imported);
            before = *version;
        }
        if revisions.len() < 20 {
            break;
        }
    }
    let mut images: Vec<PageImage> = sqlx::query_as("SELECT id,name,size_bytes,width,height,'/api/v1/page-images/' || id AS url,'/api/v1/page-images/' || id AS preview_url FROM page_images WHERE page_id=$1 ORDER BY id DESC")
        .bind(page).fetch_all(&s.pool).await?;
    if !imported.is_empty() {
        let ids: Vec<i64> = imported.into_iter().collect();
        let legacy: Vec<PageImage> = sqlx::query_as("SELECT a.id,a.name,a.size_bytes,NULL::integer AS width,NULL::integer AS height,'/api/v1/legacy-media/' || a.id AS url,'/api/v1/admin/legacy-media/' || a.id AS preview_url FROM attachments a WHERE a.id=ANY($1) AND a.removed_at IS NULL AND a.data IS NOT NULL AND a.content_type IN ('image/png','image/jpeg','image/webp','image/gif') AND EXISTS(SELECT 1 FROM legacy_sources s WHERE s.attachment_id=a.id) ORDER BY a.id")
            .bind(ids).fetch_all(&s.pool).await?;
        images.extend(legacy);
    }
    Ok(Json(images))
}

fn collect_imported(content: &str, ids: &mut BTreeSet<i64>) {
    for url in crate::markdown::image_urls(content) {
        if let Some(id) = url
            .strip_prefix("/api/v1/legacy-media/")
            .and_then(|id| id.parse().ok())
        {
            ids.insert(id);
        }
    }
}

pub async fn upload(
    State(s): State<Backend>,
    admin: Admin,
    Path(page): Path<i64>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<PageImage>)> {
    let field = multipart
        .next_field()
        .await
        .map_err(|_| bad("Neplatné nahrání obrázku."))?
        .ok_or_else(|| bad("Vyberte obrázek."))?;
    if field.name() != Some("file") {
        return Err(bad("Očekáváno pole file."));
    }
    let name = field
        .file_name()
        .ok_or_else(|| bad("Chybí název souboru."))?
        .to_owned();
    if name.trim().is_empty()
        || name.len() > 180
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(bad("Neplatný název souboru."));
    }
    let bytes = field
        .bytes()
        .await
        .map_err(|_| bad("Obrázek nelze přečíst nebo překračuje limit 10 MiB."))?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err(bad("Obrázek musí mít 1 bajt až 10 MiB."));
    }
    if multipart
        .next_field()
        .await
        .map_err(|_| bad("Neplatné nahrání obrázku."))?
        .is_some()
    {
        return Err(bad("Nahrajte jeden obrázek v jednom požadavku."));
    }
    let image_bytes = bytes.clone();
    let (mime, width, height) = tokio::task::spawn_blocking(move || validate(&image_bytes))
        .await
        .map_err(|_| bad("Obrázek nelze zpracovat."))??;
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    // Lock the parent to serialize simultaneous uploads and enforce the page limit.
    sqlx::query_scalar::<_, i64>("SELECT id FROM pages WHERE id=$1 FOR UPDATE")
        .bind(page)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM page_images WHERE page_id=$1")
        .bind(page)
        .fetch_one(&mut *tx)
        .await?;
    if count >= 100 {
        return Err(bad("Stránka může mít nejvýše 100 nahraných obrázků."));
    }
    let image: PageImage = sqlx::query_as("INSERT INTO page_images(page_id,name,content_type,size_bytes,width,height,data,sha256,created_at,actor_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id,name,size_bytes,width,height,'/api/v1/page-images/' || id AS url,'/api/v1/page-images/' || id AS preview_url")
        .bind(page).bind(name).bind(mime).bind(bytes.len() as i64).bind(width as i32).bind(height as i32)
        .bind(bytes.as_ref()).bind(super::auth::hash(&bytes)).bind(timestamp(now)).bind(admin.id)
        .fetch_one(&mut *tx).await?;
    audit(
        &mut tx,
        Some(admin.id),
        "uploaded",
        "page_image",
        image.id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(image)))
}

fn validate(bytes: &[u8]) -> Result<(&'static str, u32, u32)> {
    let format = image::guess_format(bytes)
        .map_err(|_| bad("Podporované obrázky: PNG, JPEG, WebP a GIF."))?;
    let mime = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        _ => return Err(bad("Podporované obrázky: PNG, JPEG, WebP a GIF.")),
    };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| bad("Obrázek je poškozený nebo příliš velký. Nejvýše 8192 × 8192 bodů a 128 MiB po rozbalení."))?;
    Ok((mime, image.width(), image.height()))
}

pub async fn media(
    State(s): State<Backend>,
    Path(id): Path<i64>,
    admin: std::result::Result<Admin, super::Error>,
) -> Result<Response> {
    let row: Option<(bool, String)> = sqlx::query_as("SELECT p.published,p.content FROM pages p JOIN page_images i ON i.page_id=p.id WHERE i.id=$1")
        .bind(id).fetch_optional(&s.pool).await?;
    let (published, content) = row.ok_or_else(missing)?;
    if admin.is_err()
        && (!published
            || !crate::markdown::contains_image(&content, &format!("/api/v1/page-images/{id}")))
    {
        return Err(missing());
    }
    let (mime, bytes): (String, Vec<u8>) =
        sqlx::query_as("SELECT content_type,data FROM page_images WHERE id=$1")
            .bind(id)
            .fetch_one(&s.pool)
            .await?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_DISPOSITION, "inline".into()),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'".into(),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}
