//! Calendar contracts shared by the API, public views, and administration.
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const EVENT_TITLE_LIMIT: usize = 200;
pub const EVENT_LOCATION_LIMIT: usize = 300;
pub const EVENT_DESCRIPTION_LIMIT: usize = 10_000;
pub const EVENT_PAGE_SIZE: i64 = 20;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub location: String,
    /// RFC3339 with the Europe/Prague offset at the event's instant.
    pub starts_at: String,
    pub ends_at: String,
    pub published: bool,
    pub cancelled: bool,
    pub version: i64,
    pub updated_at: String,
}

impl Event {
    pub fn href(&self) -> String {
        format!("/kalendar/{}", self.id)
    }

    pub fn start_label(&self) -> String {
        date_label(&self.starts_at)
    }

    pub fn end_label(&self) -> String {
        date_label(&self.ends_at)
    }
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
