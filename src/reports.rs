//! Contracts shared by public fault reports, their API and administration.
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

pub const REPORT_DESCRIPTION_LIMIT: usize = 5_000;
pub const REPORT_ADDRESS_LIMIT: usize = 300;
pub const REPORT_NAME_LIMIT: usize = 120;
pub const REPORT_PAGE_SIZE: i64 = 20;
pub const REPORT_PHOTO_BYTES: usize = 20_000_000;
pub const REPORT_UPLOAD_BYTES: usize = 50_000_000;
pub const REPORT_SEARCH_LIMIT: usize = 200;
pub const REPORT_MAP_LIMIT: i64 = 2_000;
pub const REPORT_RESOLUTION_LIMIT: usize = 5_000;

/// Public overview selection. Resolved reports remain directly accessible by URL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatusFilter {
    #[default]
    Unresolved,
    All,
    Reported,
    InProgress,
    Resolved,
}

impl ReportStatusFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::All => "all",
            Self::Reported => "reported",
            Self::InProgress => "in_progress",
            Self::Resolved => "resolved",
        }
    }

    pub fn from_query(value: &str) -> Option<Self> {
        match value {
            "unresolved" => Some(Self::Unresolved),
            "all" => Some(Self::All),
            "reported" => Some(Self::Reported),
            "in_progress" => Some(Self::InProgress),
            "resolved" => Some(Self::Resolved),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unresolved => "Nevyřešené",
            Self::All => "Všechny",
            Self::Reported => "Nahlášené",
            Self::InProgress => "V řešení",
            Self::Resolved => "Vyřešené",
        }
    }

    #[cfg(feature = "ssr")]
    pub(crate) fn statuses(self) -> &'static [&'static str] {
        match self {
            Self::Unresolved => &["reported", "in_progress"],
            Self::All => &["reported", "in_progress", "resolved"],
            Self::Reported => &["reported"],
            Self::InProgress => &["in_progress"],
            Self::Resolved => &["resolved"],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
pub struct Report {
    pub id: i64,
    pub description: String,
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub created_at: String,
    pub status: String,
    pub has_photo: bool,
    #[serde(default)]
    pub photo_count: i64,
    #[serde(default)]
    pub resolution_comment: Option<String>,
    #[serde(default)]
    pub resolution_updated_at: Option<String>,
    #[serde(default)]
    pub resolution_photo_ids: Vec<i64>,
}

impl Report {
    pub fn href(&self) -> String {
        format!("/hlaseni-zavad/{}", self.id)
    }

    pub fn photo_url(&self) -> String {
        format!("/api/v1/reports/{}/photo", self.id)
    }

    pub fn photo_urls(&self) -> Vec<String> {
        (1..=self.photo_count)
            .map(|position| format!("/api/v1/reports/{}/photos/{position}", self.id))
            .collect()
    }

    pub fn resolution_photo_urls(&self) -> Vec<String> {
        self.resolution_photo_ids
            .iter()
            .map(|photo_id| format!("/api/v1/reports/{}/resolution-photos/{photo_id}", self.id))
            .collect()
    }

    pub fn status_label(&self) -> &'static str {
        status_label(&self.status)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
pub struct ReportMapPoint {
    pub id: i64,
    pub latitude: f64,
    pub longitude: f64,
    pub description: String,
    pub address: Option<String>,
    pub created_at: String,
    pub status: String,
    pub has_photo: bool,
}

impl ReportMapPoint {
    pub fn href(&self) -> String {
        format!("/hlaseni-zavad/{}", self.id)
    }

    pub fn photo_url(&self) -> String {
        format!("/api/v1/reports/{}/photo", self.id)
    }

    pub fn status_label(&self) -> &'static str {
        status_label(&self.status)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportMap {
    pub points: Vec<ReportMapPoint>,
    pub located_count: i64,
    pub unlocated_count: i64,
    pub truncated: bool,
}

/// A compact first-sentence preview, bounded in characters rather than UTF-8 bytes.
pub fn report_excerpt(description: &str) -> String {
    const LIMIT: usize = 120;
    const ABBREVIATIONS: &[&str] = &[
        "č", "p", "např", "tj", "tzn", "tzv", "atd", "apod", "resp", "mj", "ul", "sv", "ing",
        "mgr", "dr", "prof", "doc",
    ];
    let text = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = text.chars().collect::<Vec<_>>();
    let sentence_end = chars.iter().enumerate().find_map(|(index, ch)| {
        if *ch != '.' || index == 0 || index + 1 == chars.len() {
            return None;
        }
        // Decimal coordinates and dates such as 25. 10. are not sentence ends.
        if chars[index - 1].is_ascii_digit()
            && chars[index + 1..]
                .iter()
                .find(|ch| !ch.is_whitespace())
                .is_some_and(char::is_ascii_digit)
        {
            return None;
        }
        let mut word = chars[..index]
            .iter()
            .rev()
            .take_while(|ch| ch.is_alphabetic())
            .copied()
            .collect::<Vec<_>>();
        word.reverse();
        let word = word.into_iter().collect::<String>().to_lowercase();
        if ABBREVIATIONS.contains(&word.as_str()) {
            return None;
        }
        Some(index)
    });
    if sentence_end.is_none() && chars.len() <= LIMIT {
        return text;
    }
    let end = sentence_end.unwrap_or(chars.len());
    let mut cut = end.min(LIMIT - 3);
    if cut < end && chars.get(cut).is_some_and(|ch| !ch.is_whitespace()) {
        if let Some(space) = chars[..cut].iter().rposition(|ch| ch.is_whitespace()) {
            cut = space;
        }
    }
    let prefix = chars[..cut].iter().collect::<String>();
    format!(
        "{}...",
        prefix.trim_end_matches(|ch: char| ch.is_whitespace() || ch == '.')
    )
}

pub fn status_label(status: &str) -> &'static str {
    match status {
        "in_progress" => "V řešení",
        "resolved" => "Vyřešeno",
        _ => "Nahlášeno",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
pub struct AdminReport {
    #[serde(flatten)]
    #[cfg_attr(feature = "ssr", sqlx(flatten))]
    pub report: Report,
    pub request_ip: String,
    pub reporter_name: Option<String>,
    pub reporter_email: Option<String>,
    pub reporter_phone: Option<String>,
    pub request_user_agent: Option<String>,
    pub request_accept_language: Option<String>,
    pub client_timezone: Option<String>,
    pub browser_id_hash: Option<String>,
    pub browser_fingerprint_hash: Option<String>,
    pub authenticated_admin_id: Option<i64>,
    pub authenticated_admin_email: Option<String>,
    pub metadata_recorded_at: Option<i64>,
    pub metadata_expires_at: Option<i64>,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSettings {
    pub recipients: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportUpdate {
    pub status: Option<String>,
    pub hidden: Option<bool>,
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_reports(
    limit: i64,
    offset: i64,
    q: String,
    status: ReportStatusFilter,
) -> Result<Vec<Report>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    crate::backend::reports::public_reports_from_pool(&pool, limit, offset, &q, status)
        .await
        .map_err(|error| ServerFnError::new(error.1))
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_report(id: i64) -> Result<Option<Report>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    crate::backend::reports::public_report_from_pool(&pool, id)
        .await
        .map_err(|error| ServerFnError::new(error.1))
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_report_map(
    q: String,
    status: ReportStatusFilter,
) -> Result<ReportMap, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    crate::backend::reports::public_report_map_from_pool(&pool, &q, status)
        .await
        .map_err(|error| ServerFnError::new(error.1))
}

#[cfg(feature = "demo")]
pub async fn load_reports(
    _limit: i64,
    _offset: i64,
    _q: String,
    _status: ReportStatusFilter,
) -> Result<Vec<Report>, ServerFnError> {
    Ok(Vec::new())
}

#[cfg(feature = "demo")]
pub async fn load_report(_id: i64) -> Result<Option<Report>, ServerFnError> {
    Ok(None)
}

#[cfg(feature = "demo")]
pub async fn load_report_map(
    _q: String,
    _status: ReportStatusFilter,
) -> Result<ReportMap, ServerFnError> {
    Ok(ReportMap::default())
}

#[cfg(test)]
mod excerpt_tests {
    use super::report_excerpt;

    #[test]
    fn preserves_short_text_and_normalizes_whitespace() {
        assert_eq!(
            report_excerpt("  Rozbitá   lavička\n u návsi.  "),
            "Rozbitá lavička u návsi."
        );
        assert_eq!(report_excerpt("Nefunguje osvětlení"), "Nefunguje osvětlení");
        assert_eq!(report_excerpt(""), "");
    }

    #[test]
    fn shows_only_the_first_sentence_with_one_ellipsis() {
        assert_eq!(
            report_excerpt("Rozbitá lavička u návsi. Uvolněná opěrka lavičky."),
            "Rozbitá lavička u návsi..."
        );
    }

    #[test]
    fn bounds_unicode_text_and_prefers_a_word_boundary() {
        let excerpt = report_excerpt(&"ž".repeat(121));
        assert_eq!(excerpt.chars().count(), 120);
        assert_eq!(excerpt, format!("{}...", "ž".repeat(117)));
        let words = "Poškozená lavička potřebuje opravit ".repeat(8);
        let excerpt = report_excerpt(&words);
        assert!(excerpt.chars().count() <= 120);
        assert!(words.starts_with(excerpt.trim_end_matches('.')));
        assert!(words[excerpt.trim_end_matches('.').len()..].starts_with(' '));
    }

    #[test]
    fn keeps_decimal_coordinates_and_common_czech_abbreviations() {
        assert_eq!(
            report_excerpt("U č. p. 50, např. u vchodu, tj. vlevo. Další podrobnosti."),
            "U č. p. 50, např. u vchodu, tj. vlevo..."
        );
        assert_eq!(
            report_excerpt("Díra na 50.529509, 15.158555 má šířku 1.5 m. Je nebezpečná."),
            "Díra na 50.529509, 15.158555 má šířku 1.5 m..."
        );
    }
}
