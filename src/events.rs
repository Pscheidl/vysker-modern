//! Calendar contracts shared by the API, public views, and administration.
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const EVENT_TITLE_LIMIT: usize = 200;
pub const EVENT_LOCATION_LIMIT: usize = 300;
pub const EVENT_DESCRIPTION_LIMIT: usize = 10_000;
pub const EVENT_PAGE_SIZE: i64 = 20;
pub const EVENT_CATEGORIES: &[(&str, &str)] = &[
    ("other", "Ostatní"),
    ("waste", "Svozy odpadu"),
    ("culture", "Kulturní a společenské akce"),
    ("council", "Zasedání zastupitelstva"),
];

pub fn default_category() -> String {
    "other".into()
}

pub fn category_label(category: &str) -> &'static str {
    EVENT_CATEGORIES
        .iter()
        .find(|(key, _)| *key == category)
        .map(|(_, label)| *label)
        .unwrap_or("Ostatní")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub location: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub repeat_every_14_days: bool,
    /// Inclusive last allowed start date, in Europe/Prague.
    #[serde(default)]
    pub repeat_until: Option<String>,
    /// Public display only. Administration and exports retain the original anchor.
    #[serde(default)]
    pub occurrence_starts_at: Option<String>,
    #[serde(default)]
    pub occurrence_ends_at: Option<String>,
    /// RFC3339 with the Europe/Prague offset at the event's instant.
    pub starts_at: String,
    pub ends_at: String,
    pub start_time_known: bool,
    pub end_time_known: bool,
    pub end_date_known: bool,
    pub published: bool,
    pub cancelled: bool,
    pub version: i64,
    pub updated_at: String,
    #[serde(default)]
    pub source_provider: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
}

impl Event {
    pub fn href(&self) -> String {
        format!("/kalendar/{}", self.id)
    }

    pub fn display_start(&self) -> &str {
        self.occurrence_starts_at
            .as_deref()
            .unwrap_or(&self.starts_at)
    }

    pub fn display_end(&self) -> &str {
        self.occurrence_ends_at.as_deref().unwrap_or(&self.ends_at)
    }

    pub fn recurrence_label(&self) -> Option<String> {
        if !self.repeat_every_14_days {
            return None;
        }
        let mut label = "Opakuje se každých 14 dní".to_owned();
        if let Some(until) = &self.repeat_until {
            let date = time::Date::parse(
                until,
                time::macros::format_description!("[year]-[month]-[day]"),
            );
            let until = date
                .map(|date| {
                    format!(
                        "{}. {}. {}",
                        date.day(),
                        u8::from(date.month()),
                        date.year()
                    )
                })
                .unwrap_or_else(|_| until.clone());
            label.push_str(&format!(", nejpozději do {until} včetně"));
        }
        Some(label)
    }

    pub fn start_label(&self) -> String {
        if self.all_day {
            return format!("{} (celý den)", date_only_label(self.display_start()));
        }
        precise_label(self.display_start(), self.start_time_known)
    }

    pub fn end_label(&self) -> String {
        if self.all_day {
            return format!("{} (celý den)", date_only_label(self.display_end()));
        }
        if !self.end_date_known {
            return "Původní web datum konce neuváděl".into();
        }
        precise_label(self.display_end(), self.end_time_known)
    }
}

pub fn date_only_label(value: &str) -> String {
    date_label(value)
        .split(" v ")
        .next()
        .unwrap_or(value)
        .to_owned()
}

fn precise_label(value: &str, known: bool) -> String {
    if known {
        return date_label(value);
    }
    let label = date_label(value);
    format!(
        "{} (původní web čas neuváděl)",
        label.split(" v ").next().unwrap_or(&label)
    )
}

pub fn date_label(value: &str) -> String {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(|date| {
            format!(
                "{}. {}. {} v {:02}:{:02}",
                date.day(),
                u8::from(date.month()),
                date.year(),
                date.hour(),
                date.minute()
            )
        })
        .unwrap_or_else(|_| value.into())
}

/// Preserve seconds and fractional seconds when editing a stored instant.
pub fn local_datetime(value: &str) -> String {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|date| {
            date.format(time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond]"
            ))
            .ok()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub location: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub repeat_every_14_days: bool,
    #[serde(default)]
    pub repeat_until: Option<String>,
    pub starts_at: String,
    pub ends_at: String,
    #[serde(default)]
    pub published: bool,
    #[serde(default)]
    pub cancelled: bool,
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDelete {
    pub expected_version: i64,
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_events(
    archived: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<Event>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    crate::backend::events::public_events_from_pool(
        &pool,
        archived,
        limit,
        offset,
        OffsetDateTime::now_utc(),
    )
    .await
    .map_err(|error| ServerFnError::new(error.1))
}

#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_event(id: i64) -> Result<Option<Event>, ServerFnError> {
    let pool = use_context::<sqlx::PgPool>()
        .ok_or_else(|| ServerFnError::new("Databáze není dostupná."))?;
    crate::backend::events::public_event_from_pool(&pool, id)
        .await
        .map_err(|error| ServerFnError::new(error.1))
}

#[cfg(feature = "demo")]
pub async fn load_events(
    _archived: bool,
    _limit: i64,
    _offset: i64,
) -> Result<Vec<Event>, ServerFnError> {
    Ok(Vec::new())
}

#[cfg(feature = "demo")]
pub async fn load_event(_id: i64) -> Result<Option<Event>, ServerFnError> {
    Ok(None)
}

/// Canonical URL from configuration, never from an untrusted Host header.
#[cfg(not(feature = "demo"))]
#[server]
pub async fn load_calendar_url() -> Result<String, ServerFnError> {
    let backend = use_context::<crate::backend::Backend>()
        .ok_or_else(|| ServerFnError::new("Kalendář není dostupný."))?;
    Ok(format!("{}/api/v1/calendar.ics", backend.config.public_url))
}

#[cfg(feature = "demo")]
pub async fn load_calendar_url() -> Result<String, ServerFnError> {
    Ok(String::new())
}
