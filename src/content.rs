//! Datové kontrakty veřejného rozhraní. SQL a databáze se do WASM nepřenášejí.
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub size: String,
    pub removed: bool,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ImportOrigin {
    pub source_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Notice {
    pub id: i64,
    pub title: String,
    pub category: String,
    pub reference: String,
    pub issuer: String,
    pub description: String,
    pub posted: String,
    pub posted_iso: String,
    pub ends: String,
    pub ends_iso: Option<String>,
    pub archived: bool,
    pub retain_files: bool,
    pub remaining: Option<i64>,
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub import_origin: Option<ImportOrigin>,
}

impl Notice {
    pub fn href(&self) -> String {
        format!("/uredni-deska/{}", self.id)
    }
    pub fn matches(&self, query: &str) -> bool {
        let haystack = fold(&format!(
            "{} {} {} {} {}",
            self.title, self.category, self.reference, self.issuer, self.description
        ));
        fold(query)
            .split_whitespace()
            .all(|word| haystack.contains(word))
    }
    pub fn has_files(&self) -> bool {
        (!self.archived || self.retain_files) && self.attachments.iter().any(|f| !f.removed)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Board {
    pub notices: Vec<Notice>,
    pub today: String,
    #[serde(default)]
    pub published_count: usize,
}

pub fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' => 'a',
            'č' => 'c',
            'ď' => 'd',
            'é' | 'ě' => 'e',
            'í' => 'i',
            'ň' => 'n',
            'ó' => 'o',
            'ř' => 'r',
            'š' => 's',
            'ť' => 't',
            'ú' | 'ů' => 'u',
            'ý' => 'y',
            'ž' => 'z',
            _ => c,
        })
        .collect()
}

#[cfg(feature = "ssr")]
fn short_date(date: time::Date) -> String {
    format!("{}. {}. {}", date.day(), date.month() as u8, date.year())
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_board() -> Result<Board, ServerFnError> {
    use crate::model::notices;
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Připojení k dokumentům není dostupné."))?;
    let today = crate::backend::today(time::OffsetDateTime::now_utc());
    let months = [
        "ledna",
        "února",
        "března",
        "dubna",
        "května",
        "června",
        "července",
        "srpna",
        "září",
        "října",
        "listopadu",
        "prosince",
    ];
    let mut records = notices::published(&pool, 3).await.map_err(public_error)?;
    records.extend(notices::archived(&pool, 3).await.map_err(public_error)?);
    let mut notices = Vec::with_capacity(records.len());
    for record in records {
        notices.push(present_notice(&pool, record).await?);
    }
    let counts = crate::model::notices::counts(&pool)
        .await
        .map_err(public_error)?;
    Ok(Board {
        published_count: counts.published as usize,
        notices,
        today: format!(
            "{}. {} {}",
            today.day(),
            months[today.month() as usize - 1],
            today.year()
        ),
    })
}

#[cfg(feature = "ssr")]
async fn present_notice(
    pool: &sqlx::PgPool,
    record: crate::model::notices::Notice,
) -> Result<Notice, ServerFnError> {
    let today = crate::backend::today(time::OffsetDateTime::now_utc());
    let files = crate::model::notices::attachments(pool, record.id)
        .await
        .map_err(public_error)?;
    let archived = !record.status.is_published();
    let end_date = if archived {
        record.withdrawn_at.map(|d| d.date()).or(record.withdraw_on)
    } else {
        record.withdraw_on
    };
    Ok(Notice {
        id: record.id,
        title: record.title,
        category: record.category_name.unwrap_or_else(|| "Ostatní".into()),
        reference: record
            .reference_number
            .unwrap_or_else(|| "Neuvedeno".into()),
        issuer: record.issuer.unwrap_or_else(|| "Obec Vyskeř".into()),
        description: record.description.unwrap_or_default(),
        posted: record
            .published_on
            .map(short_date)
            .unwrap_or_else(|| "Datum vyvěšení není uvedeno".into()),
        posted_iso: record
            .published_on
            .map(|d| d.to_string())
            .unwrap_or_default(),
        ends: end_date
            .map(short_date)
            .unwrap_or_else(|| "Datum sejmutí není uvedeno".into()),
        ends_iso: end_date.map(|d| d.to_string()),
        archived,
        retain_files: record.retain_attachments,
        remaining: record.withdraw_on.map(|d| (d - today).whole_days()),
        import_origin: None,
        attachments: files
            .into_iter()
            .map(|file| Attachment {
                name: file.name,
                size: if file.size_bytes < 1024 {
                    format!("{} B", file.size_bytes)
                } else if file.size_bytes >= 1_048_576 {
                    format!("{:.1} MB", file.size_bytes as f64 / 1_048_576.0)
                } else {
                    format!("{} kB", (file.size_bytes + 1023) / 1024)
                },
                removed: file.removed_at.is_some() || (archived && !record.retain_attachments),
                url: if file.has_content
                    && file.removed_at.is_none()
                    && (!archived || record.retain_attachments)
                {
                    Some(format!("/api/v1/attachments/{}", file.id))
                } else {
                    None
                },
            })
            .collect(),
    })
}

#[cfg(feature = "ssr")]
pub async fn notice_from_pool(
    pool: &sqlx::PgPool,
    id: i64,
) -> Result<Option<Notice>, ServerFnError> {
    match crate::model::notices::by_id(pool, id)
        .await
        .map_err(public_error)?
    {
        Some(record) => {
            let mut notice = present_notice(pool, record).await?;
            notice.import_origin = crate::backend::legacy::import_origin(
                pool,
                Some(id),
                None,
                !notice.archived || notice.retain_files,
            )
            .await
            .map_err(public_error)?;
            Ok(Some(notice))
        }
        None => Ok(None),
    }
}
#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_notice(id: i64) -> Result<Option<Notice>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    notice_from_pool(&pool, id).await
}
#[cfg(feature = "demo")]
pub async fn load_notice(id: i64) -> Result<Option<Notice>, ServerFnError> {
    Ok(load_board().await?.notices.into_iter().find(|n| n.id == id))
}

/// Veřejná ukázka obsahuje pouze tato verzovaná data, nikdy lokální databázi.
#[cfg(feature = "demo")]
pub async fn load_board() -> Result<Board, ServerFnError> {
    serde_json::from_str(include_str!("../demo/documents.json"))
        .map_err(|_| ServerFnError::new("Dokumenty se nepodařilo načíst."))
}

#[cfg(feature = "ssr")]
fn public_error(error: sqlx::Error) -> ServerFnError {
    tracing::error!(%error, "načtení úřední desky selhalo");
    ServerFnError::new("Dokumenty se nepodařilo načíst. Zkuste to prosím znovu.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice() -> Notice {
        Notice {
            id: 6,
            title: "Záměr pronájmu obecního pozemku".into(),
            category: "Záměr obce".into(),
            reference: "OUV-405/2026".into(),
            issuer: "Obec Vyskeř".into(),
            description: "Nabídky lze podávat po dobu vyvěšení.".into(),
            posted: "20. 9. 2026".into(),
            posted_iso: "2026-09-20".into(),
            ends: "5. 10. 2026".into(),
            ends_iso: Some("2026-10-05".into()),
            archived: false,
            retain_files: false,
            remaining: Some(6),
            import_origin: None,
            attachments: vec![Attachment {
                name: "Záměr obce".into(),
                size: "96 kB".into(),
                removed: false,
                url: None,
            }],
        }
    }

    #[test]
    fn search_accepts_czech_without_diacritics_and_multiple_words() {
        let n = notice();
        assert!(n.matches("  POZEMKU  zamer  "));
        assert!(n.matches("vysker ouv-405"));
        assert!(!n.matches("zamer volby"));
    }

    #[test]
    fn archive_keeps_record_but_hides_nonretained_files() {
        let mut n = notice();
        assert!(n.has_files());
        n.archived = true;
        assert!(!n.has_files());
        assert!(n.matches("pronajmu"));
        assert_eq!(n.href(), "/uredni-deska/6");
    }

    #[test]
    fn removed_attachment_stays_hidden_even_when_retention_is_enabled() {
        let mut n = notice();
        n.archived = true;
        n.retain_files = true;
        assert!(n.has_files());
        n.attachments[0].removed = true;
        assert!(!n.has_files());
    }
}
