//! Public text copies inherit the original attachment's current visibility.
use super::{Backend, Result, attachment_index::EXTRACTOR_VERSION, missing};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use sqlx::{FromRow, types::Json};
use time::OffsetDateTime;

#[derive(FromRow)]
struct TextPage {
    name: String,
    document_id: i64,
    method: String,
    warnings: Json<Vec<String>>,
    page: i32,
    label: String,
    content: String,
}

pub async fn preview(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Response> {
    serve(&s, id, false).await
}

pub async fn download(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Response> {
    serve(&s, id, true).await
}

async fn serve(s: &Backend, id: i64, download: bool) -> Result<Response> {
    // A single snapshot reads both permissions and every page. Authentication
    // deliberately does not grant an exception to these public-only routes.
    let pages = sqlx::query_as::<_, TextPage>(
        "SELECT a.name,a.document_id,e.method,e.warnings,p.page,p.label,p.content \
         FROM attachments a JOIN documents d ON d.id=a.document_id \
         JOIN attachment_extractions e ON e.attachment_id=a.id \
         JOIN attachment_search_pages p ON p.attachment_id=a.id \
         WHERE a.id=$1 AND a.data IS NOT NULL AND a.removed_at IS NULL AND a.is_public \
         AND document_files_public(d,$2) AND e.status='ready' AND e.source_name=a.name \
         AND e.extractor_version=$3 ORDER BY p.page",
    )
    .bind(id)
    .bind(super::today(OffsetDateTime::now_utc()))
    .bind(EXTRACTOR_VERSION)
    .fetch_all(&s.pool)
    .await?;
    let Some(first) = pages.first() else {
        // A private/missing/stale copy has the same public response and cannot
        // be retained by a browser cache after a visibility change.
        let mut response = missing().into_response();
        response.headers_mut().extend(protection_headers());
        return Ok(response);
    };
    let original_name = single_line(&first.name);
    let public_url = s.config.public_url.trim_end_matches('/');
    let mut text = format!(
        "Textový přepis přílohy\nSoubor: {original_name}\nDokument: {public_url}/dokumenty/{}\nOriginál: {public_url}/api/v1/attachments/{id}\n",
        first.document_id
    );
    if matches!(first.method.as_str(), "ocr" | "mixed") {
        text.push_str(
            "\nUpozornění: Automatický přepis OCR může obsahovat chyby. Pro kontrolu použijte původní soubor.\n",
        );
    }
    for warning in &first.warnings.0 {
        let warning = single_line(warning);
        if !warning.is_empty() {
            text.push_str(&format!("Upozornění při zpracování: {warning}\n"));
        }
    }
    for page in &pages {
        let label = single_line(&page.label);
        let label = if label.is_empty() {
            format!("Část {}", page.page)
        } else {
            label
        };
        text.push_str(&format!("\n=== {label} ===\n\n"));
        // This is a literal text/plain response, not HTML. Keeping angle brackets
        // intact preserves searchable XML and other original document content.
        text.push_str(&page.content);
        text.push('\n');
    }
    let mut headers = protection_headers();
    headers.insert(
        header::CONTENT_TYPE,
        "text/plain; charset=utf-8".parse().unwrap(),
    );
    let name = first
        .name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if matches!(c, '/' | '\\') { '_' } else { c })
        .take(180)
        .collect::<String>();
    let name = format!(
        "{}.txt",
        if name.trim().is_empty() {
            "dokument"
        } else {
            &name
        }
    );
    let encoded = percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC);
    let disposition = if download { "attachment" } else { "inline" };
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("{disposition}; filename=\"prepis.txt\"; filename*=UTF-8''{encoded}")
            .parse()
            .map_err(|_| missing())?,
    );
    Ok((headers, Body::from(text)).into_response())
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn protection_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, "no-store, private".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        "sandbox; default-src 'none'".parse().unwrap(),
    );
    headers.insert("x-robots-tag", "noindex, noarchive".parse().unwrap());
    headers
}
