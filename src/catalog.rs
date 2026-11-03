//! Sdílené veřejné kontrakty pro katalog a obsahové stránky.
use crate::content::{Attachment, ImportOrigin};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Missing source dates are displayed without substituting the import date.
pub fn document_date(value: &str) -> String {
    let day = value.get(..10).unwrap_or(value);
    let parts: Vec<_> = day.split('-').collect();
    if parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    {
        format!(
            "{}. {}. {}",
            parts[2].trim_start_matches('0'),
            parts[1].trim_start_matches('0'),
            parts[0]
        )
    } else {
        "Datum není uvedeno".into()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub files: Vec<Attachment>,
    #[serde(default)]
    pub reference_number: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub document_date: Option<String>,
    #[serde(default)]
    pub source_published_on: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub postings: Vec<DocumentPosting>,
    #[serde(default)]
    pub relations: Vec<DocumentRelation>,
    #[serde(default)]
    pub import_origin: Option<ImportOrigin>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentPosting {
    pub id: i64,
    #[serde(default)]
    pub initial_state: String,
    pub origin: String,
    pub certainty: String,
    pub started_on: Option<String>,
    pub started_at: Option<String>,
    pub planned_removal_on: Option<String>,
    pub removed_on: Option<String>,
    pub removed_at: Option<String>,
    #[serde(default)]
    pub corrections: Vec<PostingCorrection>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostingCorrection {
    pub id: i64,
    pub field: String,
    pub original: Option<String>,
    pub corrected: Option<String>,
    pub recorded_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
pub struct DocumentRelation {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub direction: String,
}

pub fn document_event_date(timestamp: Option<&str>, day: Option<&str>) -> String {
    timestamp
        .and_then(|value| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
        })
        .map(|value| {
            format!(
                "{}. {}. {} v {:02}:{:02} (Europe/Prague)",
                value.day(),
                value.month() as u8,
                value.year(),
                value.hour(),
                value.minute()
            )
        })
        .or_else(|| day.map(document_date))
        .unwrap_or_else(|| "Není doloženo".into())
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Page {
    pub title: String,
    pub content: String,
}

pub fn page_path(slug: &str) -> String {
    if slug == "kontakt" {
        "/obec".into()
    } else {
        format!("/stranky/{slug}")
    }
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
    let today = crate::backend::today(time::OffsetDateTime::now_utc());
    #[derive(sqlx::FromRow)]
    struct Record {
        title: String,
        description: String,
        reference_number: Option<String>,
        issuer: Option<String>,
        document_date: Option<String>,
        source_published_on: Option<String>,
        topic: Option<String>,
        files_public: bool,
    }
    let record: Option<Record> = sqlx::query_as(
        "SELECT CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Dokument #'||d.id) ELSE d.title END AS title,
         CASE WHEN d.metadata_restricted THEN '' ELSE coalesce(d.description,'') END AS description,
         CASE WHEN d.metadata_restricted THEN NULL ELSE d.reference_number END AS reference_number,
         CASE WHEN d.metadata_restricted THEN NULL ELSE d.issuer END AS issuer,
         d.document_date::text AS document_date,d.source_published_on::text AS source_published_on,
         c.name AS topic,document_files_public(d,$2) AS files_public
         FROM documents d LEFT JOIN categories c ON c.id=d.category_id
         WHERE d.id=$1 AND document_is_public(d,$2)",
    )
    .bind(id)
    .bind(today)
    .fetch_optional(pool)
    .await
    .map_err(|_| ServerFnError::new("Dokument nelze načíst."))?;
    let Some(record) = record else {
        return Ok(None);
    };
    let rows: Vec<(i64, String, i64, bool)> = sqlx::query_as(
        "SELECT id,name,size_bytes,EXISTS(SELECT 1 FROM attachment_extractions e WHERE e.attachment_id=attachments.id AND e.status='ready' AND e.extractor_version=$3 AND e.source_name=attachments.name AND EXISTS(SELECT 1 FROM attachment_search_pages p WHERE p.attachment_id=e.attachment_id)) AS text_ready FROM attachments
         WHERE document_id=$1 AND is_public AND data IS NOT NULL AND removed_at IS NULL
         AND EXISTS(SELECT 1 FROM documents d WHERE d.id=$1 AND document_files_public(d,$2))
         ORDER BY sort_order,id",
    )
    .bind(id)
    .bind(today)
    .bind(crate::backend::attachment_index::EXTRACTOR_VERSION)
    .fetch_all(pool)
    .await
    .map_err(|_| ServerFnError::new("Přílohy nelze načíst."))?;
    let files = rows
        .into_iter()
        .map(|(id, name, size, text_ready)| Attachment {
            name,
            size: if size < 1024 {
                format!("{size} B")
            } else if size < 1_048_576 {
                format!("{} kB", (size + 1023) / 1024)
            } else {
                format!("{:.1} MB", size as f64 / 1_048_576.0)
            },
            removed: false,
            url: Some(format!("/api/v1/attachments/{id}")),
            text_url: text_ready.then(|| format!("/api/v1/attachments/{id}/text")),
        })
        .collect();
    #[derive(sqlx::FromRow)]
    struct Posting {
        id: i64,
        initial_state: String,
        origin: String,
        certainty: String,
        started_on: Option<time::Date>,
        started_at: Option<time::OffsetDateTime>,
        planned_removal_on: Option<time::Date>,
        removed_on: Option<time::Date>,
        removed_at: Option<time::OffsetDateTime>,
    }
    let postings: Vec<Posting> = sqlx::query_as(
        "SELECT id,origin,certainty,initial_state,started_on,started_at,planned_removal_on,removed_on,removed_at
         FROM document_postings WHERE document_id=$1 ORDER BY started_on DESC NULLS LAST,id DESC",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(|_| ServerFnError::new("Historii vyvěšení nelze načíst."))?;
    let timestamp = |value: time::OffsetDateTime| {
        use time_tz::OffsetDateTimeExt;
        value
            .to_timezone(time_tz::timezones::db::europe::PRAGUE)
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    };
    let corrections:Vec<(i64,i64,String,Option<String>,Option<String>,time::OffsetDateTime)>=sqlx::query_as(
        "SELECT id,(payload->>'posting_id')::bigint,payload->>'field',payload->>'original',payload->>'corrected',occurred_at
         FROM document_events WHERE document_id=$1 AND kind='evidence_corrected' ORDER BY id"
    ).bind(id).fetch_all(pool).await.map_err(|_|ServerFnError::new("Opravy evidenčních údajů nelze načíst."))?;
    let postings = postings
        .into_iter()
        .map(|posting| DocumentPosting {
            id: posting.id,
            initial_state: posting.initial_state,
            origin: posting.origin,
            certainty: posting.certainty,
            started_on: posting.started_on.map(|v| v.to_string()),
            started_at: posting.started_at.map(timestamp),
            planned_removal_on: posting.planned_removal_on.map(|v| v.to_string()),
            removed_on: posting.removed_on.map(|v| v.to_string()),
            removed_at: posting.removed_at.map(timestamp),
            corrections: corrections
                .iter()
                .filter(|item| item.1 == posting.id)
                .map(|item| PostingCorrection {
                    id: item.0,
                    field: item.2.clone(),
                    original: item.3.clone(),
                    corrected: item.4.clone(),
                    recorded_at: timestamp(item.5),
                })
                .collect(),
        })
        .collect();
    let relations: Vec<DocumentRelation> = sqlx::query_as(
        "SELECT d.id,CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Dokument #'||d.id) ELSE d.title END AS title,r.kind,'following'::text AS direction FROM document_relations r
         JOIN documents d ON d.id=r.related_id WHERE r.original_id=$1 AND document_is_public(d,$2)
         UNION ALL
         SELECT d.id,CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Dokument #'||d.id) ELSE d.title END AS title,r.kind,'original'::text AS direction FROM document_relations r
         JOIN documents d ON d.id=r.original_id WHERE r.related_id=$1 AND document_is_public(d,$2)
         ORDER BY id",
    )
    .bind(id)
    .bind(today)
    .fetch_all(pool)
    .await
    .map_err(|_| ServerFnError::new("Související dokumenty nelze načíst."))?;
    Ok(Some(Document {
        id,
        title: record.title,
        description: record.description,
        files,
        reference_number: record.reference_number,
        issuer: record.issuer,
        document_date: record.document_date,
        source_published_on: record.source_published_on,
        topic: record.topic,
        postings,
        relations,
        import_origin: crate::backend::legacy::import_origin(
            pool,
            None,
            Some(id),
            record.files_public,
        )
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
pub async fn subscribe_email(
    email: String,
    fingerprint: String,
    preferences: Option<SubscriptionPreferences>,
) -> Result<(), ServerFnError> {
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
    crate::backend::subscriptions::request_subscription_with_preferences(
        &state,
        &email,
        &crate::backend::subscriptions::ConsentAcceptance { fingerprint },
        &preferences.unwrap_or_default(),
        now,
    )
    .await
    .map_err(|e| ServerFnError::new(e.1))
}
#[cfg(feature = "demo")]
pub async fn subscribe_email(
    _email: String,
    _fingerprint: String,
    _preferences: Option<SubscriptionPreferences>,
) -> Result<(), ServerFnError> {
    Err(ServerFnError::new(
        "Odběr novinek zatím není aktivní. E-mailová adresa se neukládá a zprávy se neposílají.",
    ))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionPreferences {
    pub all_notice_categories: bool,
    #[serde(default)]
    pub notice_category_ids: Vec<i64>,
    pub uncategorized_notices: bool,
    pub documents: bool,
    #[serde(default = "default_subscription_events")]
    pub events: bool,
}

fn default_subscription_events() -> bool {
    true
}

impl Default for SubscriptionPreferences {
    fn default() -> Self {
        Self {
            all_notice_categories: true,
            notice_category_ids: Vec::new(),
            uncategorized_notices: true,
            documents: true,
            events: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionCategory {
    pub id: i64,
    pub name: String,
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_subscription_categories() -> Result<Vec<SubscriptionCategory>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id,name FROM categories ORDER BY sort_order,id")
            .fetch_all(&pool)
            .await
            .map_err(|_| ServerFnError::new("Kategorie odběru nelze načíst."))?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| SubscriptionCategory { id, name })
        .collect())
}

#[cfg(feature = "demo")]
pub async fn load_subscription_categories() -> Result<Vec<SubscriptionCategory>, ServerFnError> {
    let names: std::collections::BTreeSet<_> = crate::content::load_board()
        .await?
        .notices
        .into_iter()
        .map(|notice| notice.category)
        .collect();
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(index, name)| SubscriptionCategory {
            id: index as i64 + 1,
            name,
        })
        .collect())
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_privacy_notice() -> Result<Option<crate::privacy::PrivacyNotice>, ServerFnError> {
    let state = use_context::<crate::backend::Backend>()
        .ok_or_else(|| ServerFnError::new("Informace o soukromí nejsou dostupné."))?;
    crate::backend::privacy::policy(&state)
        .await
        .map(|policy| policy.map(|p| p.notice()))
        .map_err(|error| ServerFnError::new(error.1))
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
    pub id: i64,
    pub label: String,
    pub path: String,
    pub children: Vec<NavigationLink>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SiteInfo {
    pub production: bool,
    #[serde(default)]
    pub rentals_enabled: bool,
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
    Ok(SiteInfo {
        navigation: navigation_from_pool(&state.pool).await?,
        production: state.config.production,
        rentals_enabled: crate::backend::rental_settings::enabled(&state.pool)
            .await
            .map_err(|_| ServerFnError::new("Nastavení rezervací nelze načíst."))?,
        pages: rows
            .into_iter()
            .map(|(slug, title)| PageLink { slug, title })
            .collect(),
    })
}
#[cfg(feature = "ssr")]
pub async fn navigation_from_pool(
    pool: &sqlx::PgPool,
) -> Result<Vec<NavigationLink>, ServerFnError> {
    let rows: Vec<(i64, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT id,label,path,parent_id FROM navigation_items n
         WHERE visible=TRUE
           AND ((path <> '/pronajmy' AND path NOT LIKE '/pronajmy/%')
                OR EXISTS(SELECT 1 FROM rental_settings WHERE id=1 AND enabled))
           AND (path NOT LIKE '/stranky/%' OR EXISTS(
               SELECT 1 FROM pages p WHERE n.path='/stranky/'||p.slug AND p.published=TRUE
           ))
         ORDER BY sort_order,id LIMIT 60",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| ServerFnError::new("Navigaci nelze načíst."))?;
    let mut roots = Vec::new();
    let mut children = Vec::new();
    for (id, label, path, parent_id) in rows {
        let link = NavigationLink {
            id,
            label,
            path,
            children: Vec::new(),
        };
        if let Some(parent_id) = parent_id {
            children.push((parent_id, link));
        } else {
            roots.push(link);
        }
    }
    for (parent_id, child) in children {
        // A hidden or unpublished parent must hide its entire branch.
        if let Some(parent) = roots.iter_mut().find(|root| root.id == parent_id) {
            parent.children.push(child);
        }
    }
    Ok(roots)
}
#[cfg(feature = "demo")]
pub async fn load_site_info() -> Result<SiteInfo, ServerFnError> {
    Ok(SiteInfo {
        production: false,
        rentals_enabled: false,
        pages: vec![],
        navigation: default_navigation(),
    })
}

pub fn default_navigation() -> Vec<NavigationLink> {
    [
        ("Přehled", "/"),
        ("Kalendář akcí", "/kalendar"),
        ("Úřední deska", "/uredni-deska"),
        ("Dokumenty", "/dokumenty"),
        ("Obec a úřad", "/obec"),
        ("Podněty a závady", "/hlaseni-zavad"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (label, path))| NavigationLink {
        id: -(index as i64 + 1),
        label: label.into(),
        path: path.into(),
        children: Vec::new(),
    })
    .collect()
}

#[cfg(test)]
mod document_dates_tests {
    use super::*;

    #[test]
    fn imported_day_precision_does_not_invent_an_event_time() {
        assert_eq!(document_event_date(None, Some("2015-06-14")), "14. 6. 2015");
        assert_eq!(document_event_date(None, None), "Není doloženo");
        assert_eq!(document_date(""), "Datum není uvedeno");
    }

    #[test]
    fn event_timestamp_preserves_prague_offset_and_clock_time() {
        assert_eq!(
            document_event_date(Some("2026-10-25T02:30:00+01:00"), Some("2026-10-25")),
            "25. 10. 2026 v 02:30 (Europe/Prague)"
        );
    }
}
