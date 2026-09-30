//! Resolve historical Vismo URLs only to imported, currently public records.
use super::{Backend, missing};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::collections::BTreeMap;

const PATH_SAFE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'=')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub fn source_key(path: &str, query: Option<&str>) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    if decoded.contains('\\') || decoded.chars().any(char::is_control) {
        return None;
    }
    let mut params: BTreeMap<String, String> =
        url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
            .map(|(k, v)| (k.to_lowercase(), v.into_owned()))
            .collect();
    if params.keys().any(|k| k.starts_with("xv")) {
        return None;
    }
    params.retain(|k, _| {
        !["p1", "p2", "p3", "n", "sort", "razeni", "grafika"].contains(&k.as_str())
            && !k.starts_with("utm_")
    });
    let asp = match decoded.to_ascii_lowercase().as_str() {
        "/vismo/dokumenty2.asp" => Some(("id", "d")),
        "/vismo/o_utvar.asp" => Some(("id_u", "os")),
        "/vismo/o_osoba.asp" => Some(("id_o", "o")),
        "/vismo/akce.asp" => Some(("id", "a")),
        "/vismo/galerie3.asp" => Some(("id_fotopary", "g")),
        _ => None,
    };
    if let Some((field, kind)) = asp {
        let id = params.get(field)?;
        return (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            .then(|| format!("{kind}:{id}"));
    }
    if let Some(id) = decoded.strip_prefix("/gp/id_galerie=") {
        if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
            return Some(format!("gs:{id}"));
        }
    }
    let parts: Vec<&str> = decoded
        .split('/')
        .filter(|s| !["p1=", "p2=", "p3="].iter().any(|p| s.starts_with(p)))
        .collect();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params.iter())
        .finish();
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    for (position, part) in parts.iter().enumerate() {
        if let Some((kind, id)) = part.split_once('-') {
            if ["ms", "ds", "os", "o", "d", "a", "gs", "g"].contains(&kind)
                && !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
            {
                let tail = parts[position + 1..].join("/");
                let tail = percent_encoding::utf8_percent_encode(&tail, PATH_SAFE).to_string();
                return Some(format!(
                    "{kind}:{id}{}{suffix}",
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!("/{tail}")
                    }
                ));
            }
        }
    }
    let path = parts.join("/");
    if path.starts_with("/assets/")
        || path.starts_with("/uredni-deska/")
        || path.starts_with("/vismo/")
        || ["/ap/", "/dp/", "/dsp/", "/gs/", "/gsp/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
        || ["/ap", "/dp", "/dsp", "/gs", "/gsp", "/osp"].contains(&path.as_str())
    {
        let path = percent_encoding::utf8_percent_encode(&path, PATH_SAFE);
        Some(format!("{path}{suffix}"))
    } else {
        None
    }
}

pub async fn redirects(State(state): State<Backend>, request: Request, next: Next) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD) {
        if request.uri().path().eq_ignore_ascii_case("/index.asp") {
            return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, "/")]).into_response();
        }
        if let Some(key) = source_key(request.uri().path(), request.uri().query()) {
            let found: Result<Option<String>, _> = sqlx::query_scalar(
                "SELECT s.destination FROM legacy_sources s LEFT JOIN pages p ON p.id=s.page_id LEFT JOIN documents d ON d.id=s.document_id LEFT JOIN notices n ON n.id=s.notice_id LEFT JOIN events e ON e.id=s.event_id WHERE s.source_key=$1 AND (p.published=TRUE OR d.status='published' OR n.status IN ('published','archived','withdrawn') OR e.published=TRUE)"
            ).bind(key).fetch_optional(&state.pool).await;
            match found {
                Ok(Some(target))
                    if [
                        "/stranky/",
                        "/dokumenty/",
                        "/uredni-deska/",
                        "/kalendar/",
                        "/api/v1/attachments/",
                    ]
                    .iter()
                    .any(|p| target.starts_with(p)) =>
                {
                    if let Ok(location) = target.parse::<axum::http::HeaderValue>() {
                        return (
                            StatusCode::MOVED_PERMANENTLY,
                            [(header::LOCATION, location)],
                        )
                            .into_response();
                    }
                }
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
                _ => (),
            }
        }
    }
    next.run(request).await
}

pub async fn media(State(state): State<Backend>, Path(id): Path<i64>) -> super::Result<Response> {
    let file: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT a.content_type,a.data FROM attachments a JOIN documents d ON d.id=a.document_id JOIN legacy_sources s ON s.attachment_id=a.id WHERE a.id=$1 AND d.status='published' AND a.removed_at IS NULL AND a.data IS NOT NULL AND a.content_type IN ('image/png','image/jpeg','image/gif','image/webp')"
    ).bind(id).fetch_optional(&state.pool).await?;
    let (mime, bytes) = file.ok_or_else(missing)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, mime.parse().map_err(|_| missing())?);
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        "sandbox; default-src 'none'".parse().unwrap(),
    );
    Ok((headers, Body::from(bytes)).into_response())
}
