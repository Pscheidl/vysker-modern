//! Sdílené veřejné kontrakty pro katalog a obsahové stránky.
use crate::content::{Attachment, ImportOrigin};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub files: Vec<Attachment>,
    #[serde(default)]
    pub import_origin: Option<ImportOrigin>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Page {
    pub title: String,
    pub content: String,
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_document(id: i64) -> Result<Option<Document>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    document_from_pool(&pool, id).await
}

#[cfg(feature = "ssr")]
pub async fn document_from_pool(
    pool: &sqlx::PgPool,
    id: i64,
) -> Result<Option<Document>, ServerFnError> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT title,description FROM documents WHERE id=$1 AND status='published'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|_| ServerFnError::new("Dokument nelze načíst."))?;
    let Some((title, description)) = row else {
        return Ok(None);
    };
    let rows:Vec<(i64,String,i64,bool)>=sqlx::query_as("SELECT id,name,size_bytes,data IS NOT NULL AND removed_at IS NULL FROM attachments WHERE document_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(pool).await.map_err(|_|ServerFnError::new("Přílohy nelze načíst."))?;
    let files = rows
        .into_iter()
        .map(|(id, name, size, available)| Attachment {
            name,
            size: if size < 1024 {
                format!("{size} B")
            } else if size < 1_048_576 {
                format!("{} kB", (size + 1023) / 1024)
            } else {
                format!("{:.1} MB", size as f64 / 1_048_576.0)
            },
            removed: !available,
            url: available.then(|| format!("/api/v1/attachments/{id}")),
        })
        .collect();
    Ok(Some(Document {
        id,
        title,
        description,
        files,
        import_origin: crate::backend::legacy::import_origin(pool, None, Some(id), true)
            .await
            .map_err(|_| ServerFnError::new("Původ dokumentu nelze načíst."))?,
    }))
}
#[cfg(feature = "demo")]
pub async fn load_document(_id: i64) -> Result<Option<Document>, ServerFnError> {
    Ok(None)
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_page(slug: String) -> Result<Option<Page>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT title,content FROM pages WHERE slug=$1 AND published=TRUE")
            .bind(slug)
            .fetch_optional(&pool)
            .await
            .map_err(|_| ServerFnError::new("Stránku nelze načíst."))?;
    Ok(row.map(|(title, content)| Page { title, content }))
}
#[cfg(feature = "demo")]
pub async fn load_page(_slug: String) -> Result<Option<Page>, ServerFnError> {
    Ok(None)
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn subscribe_email(email: String, fingerprint: String) -> Result<(), ServerFnError> {
    let state = use_context::<crate::backend::Backend>()
        .ok_or_else(|| ServerFnError::new("Odběr není dostupný."))?;
    let headers = leptos_axum::extract::<axum::http::HeaderMap>().await?;
    crate::backend::auth::check_origin(&headers, &state).map_err(|e| ServerFnError::new(e.1))?;
    let peer = leptos_axum::extract::<axum::extract::ConnectInfo<std::net::SocketAddr>>().await;
    let ip = crate::backend::auth::client_ip(
        peer.ok().map(|v| v.0.ip()),
        &headers,
        state.config.trusted_proxy,
    );
    let now = time::OffsetDateTime::now_utc();
    crate::backend::auth::throttle(&state, &format!("subscribe-ip:{ip}"), 20, 3600, now)
        .await
        .map_err(|e| ServerFnError::new(e.1))?;
    crate::backend::subscriptions::request_subscription(
        &state,
        &email,
        &crate::backend::subscriptions::ConsentAcceptance { fingerprint },
        now,
    )
    .await
    .map_err(|e| ServerFnError::new(e.1))
}
#[cfg(feature = "demo")]
pub async fn subscribe_email(_email: String, _fingerprint: String) -> Result<(), ServerFnError> {
    Err(ServerFnError::new(
        "Odběr novinek zatím není aktivní. E-mailová adresa se neukládá a zprávy se neposílají.",
    ))
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_privacy_notice() -> Result<Option<crate::privacy::PrivacyNotice>, ServerFnError> {
    let state = use_context::<crate::backend::Backend>()
        .ok_or_else(|| ServerFnError::new("Informace o soukromí nejsou dostupné."))?;
    Ok(state.config.privacy.as_ref().map(|p| p.notice()))
}
#[cfg(feature = "demo")]
pub async fn load_privacy_notice() -> Result<Option<crate::privacy::PrivacyNotice>, ServerFnError> {
    Ok(None)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PageLink {
    pub slug: String,
    pub title: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct NavigationLink {
    pub label: String,
    pub path: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SiteInfo {
    pub production: bool,
    pub pages: Vec<PageLink>,
    pub navigation: Vec<NavigationLink>,
}
#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_site_info() -> Result<SiteInfo, ServerFnError> {
    let state = use_context::<crate::backend::Backend>()
        .ok_or_else(|| ServerFnError::new("Nastavení webu není dostupné."))?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT slug,title FROM pages WHERE published=TRUE ORDER BY title LIMIT 1000",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|_| ServerFnError::new("Stránky nelze načíst."))?;
    let navigation:Vec<(String,String)>=sqlx::query_as("SELECT label,path FROM navigation_items n WHERE visible=TRUE AND (path NOT LIKE '/stranky/%' OR EXISTS(SELECT 1 FROM pages p WHERE n.path='/stranky/'||p.slug AND p.published=TRUE)) ORDER BY sort_order,id LIMIT 20").fetch_all(&state.pool).await.map_err(|_|ServerFnError::new("Navigaci nelze načíst."))?;
    Ok(SiteInfo {
        navigation: navigation
            .into_iter()
            .map(|(label, path)| NavigationLink { label, path })
            .collect(),
        production: state.config.production,
        pages: rows
            .into_iter()
            .map(|(slug, title)| PageLink { slug, title })
            .collect(),
    })
}
#[cfg(feature = "demo")]
pub async fn load_site_info() -> Result<SiteInfo, ServerFnError> {
    Ok(SiteInfo {
        production: false,
        pages: vec![],
        navigation: default_navigation(),
    })
}

pub fn default_navigation() -> Vec<NavigationLink> {
    [
        ("Přehled", "/"),
        ("Úřední deska", "/uredni-deska"),
        ("Dokumenty", "/dokumenty"),
        ("Obec a úřad", "/obec"),
    ]
    .into_iter()
    .map(|(label, path)| NavigationLink {
        label: label.into(),
        path: path.into(),
    })
    .collect()
}
