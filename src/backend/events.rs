//! Authenticated calendar mutations and publication-aware public queries.
use super::{Backend, Pagination, Result, audit, auth::Admin, bad, conflict, missing, timestamp};
use crate::events::{
    EVENT_DESCRIPTION_LIMIT, EVENT_LOCATION_LIMIT, EVENT_TITLE_LIMIT, Event, EventDelete,
    EventInput,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use sqlx::PgPool;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset, format_description::well_known::Rfc3339};
use time_tz::{
    Offset, OffsetDateTimeExt, OffsetResult, PrimitiveDateTimeExt, TimeZone,
    timezones::db::europe::PRAGUE,
};

#[derive(sqlx::FromRow)]
struct EventRecord {
    id: i64,
    title: String,
    description: String,
    location: String,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    start_time_known: bool,
    end_time_known: bool,
    end_date_known: bool,
    published: bool,
    cancelled: bool,
    version: i64,
    updated_at: String,
}

impl From<EventRecord> for Event {
    fn from(record: EventRecord) -> Self {
        Self {
            id: record.id,
            title: record.title,
            description: record.description,
            location: record.location,
            starts_at: timestamp(record.starts_at.to_timezone(PRAGUE)),
            ends_at: timestamp(record.ends_at.to_timezone(PRAGUE)),
            start_time_known: record.start_time_known,
            end_time_known: record.end_time_known,
            end_date_known: record.end_date_known,
            published: record.published,
            cancelled: record.cancelled,
            version: record.version,
            updated_at: record.updated_at,
        }
    }
}

struct ValidatedEvent {
    title: String,
    description: String,
    location: String,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
}

fn bounded_text(value: &str, limit: usize, required: bool, multiline: bool) -> Result<String> {
    let normalized = value.replace("\r\n", "\n");
    if normalized.chars().count() > limit
        || (required && normalized.trim().is_empty())
        || normalized
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\t')))
    {
        return Err(bad(
            "Text je prázdný, příliš dlouhý nebo obsahuje nepovolené znaky.",
        ));
    }
    Ok(normalized.trim().to_owned())
}

/// Local inputs use Prague rules. An explicit RFC3339 offset identifies one instant.
pub fn parse_event_datetime(value: &str) -> Result<OffsetDateTime> {
    if value.len() > 64 {
        return Err(bad("Neplatné datum a čas."));
    }
    let value = value.trim();
    let date = if let Ok(date) = OffsetDateTime::parse(value, &Rfc3339) {
        date
    } else {
        let local = PrimitiveDateTime::parse(
            value,
            time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]"),
        )
        .or_else(|_| {
            PrimitiveDateTime::parse(
                value,
                time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"),
            )
        })
        .or_else(|_| {
            PrimitiveDateTime::parse(
                value,
                time::macros::format_description!(
                    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond]"
                ),
            )
        })
        .map_err(|_| bad("Zadejte platné datum a čas nebo čas včetně UTC posunu."))?;
        if !(1..=9999).contains(&local.year()) {
            return Err(bad("Rok musí být v rozmezí 1 až 9999."));
        }
        match local.assume_timezone(PRAGUE) {
            OffsetResult::Some(date) => date,
            OffsetResult::Ambiguous(_, _) => {
                return Err(bad(
                    "Tento čas nastává při změně na zimní čas dvakrát. Zadejte čas s UTC posunem, například +02:00 nebo +01:00.",
                ));
            }
            OffsetResult::None => {
                return Err(bad(
                    "Tento místní čas při přechodu na letní čas neexistuje. Zvolte jiný čas nebo uveďte UTC posun.",
                ));
            }
        }
    };
    let utc = date
        .checked_to_offset(UtcOffset::UTC)
        .ok_or_else(|| bad("Datum přesahuje podporovaný rozsah."))?;
    let prague = date
        .checked_to_offset(PRAGUE.get_offset_utc(&date).to_utc())
        .ok_or_else(|| bad("Datum přesahuje podporovaný rozsah."))?;
    if !(1..=9999).contains(&date.year()) || !(1..=9999).contains(&prague.year()) || utc.year() < 1
    {
        return Err(bad("Rok musí být v rozmezí 1 až 9999."));
    }
    if prague.format(&Rfc3339).is_err() {
        return Err(bad(
            "Toto historické datum nelze zapsat s podporovaným časovým posunem.",
        ));
    }
    Ok(date)
}

fn validate(input: &EventInput) -> Result<ValidatedEvent> {
    let title = bounded_text(&input.title, EVENT_TITLE_LIMIT, true, false)?;
    let location = bounded_text(&input.location, EVENT_LOCATION_LIMIT, false, false)?;
    let description = bounded_text(&input.description, EVENT_DESCRIPTION_LIMIT, false, true)?;
    let starts_at = parse_event_datetime(&input.starts_at)?;
    let ends_at = parse_event_datetime(&input.ends_at)?;
    if ends_at < starts_at {
        return Err(bad("Konec akce nesmí být před začátkem."));
    }
    Ok(ValidatedEvent {
        title,
        description,
        location,
        starts_at,
        ends_at,
    })
}

fn expected_version(version: Option<i64>) -> Result<i64> {
    version
        .filter(|v| *v > 0 && *v < i64::MAX)
        .ok_or_else(|| bad("Chybí platná verze akce. Načtěte ji znovu."))
}

async fn version_mismatch(conn: &mut sqlx::PgConnection, id: i64) -> Result<super::Error> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id=$1)")
        .bind(id)
        .fetch_one(conn)
        .await?;
    Ok(if exists {
        conflict(
            "Akci mezitím změnil jiný správce. Vaše úpravy zůstaly ve formuláři. Načtěte aktuální verzi a změny porovnejte.",
        )
    } else {
        missing()
    })
}

pub async fn create(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<EventInput>,
) -> Result<(StatusCode, Json<Event>)> {
    let value = validate(&input)?;
    if input.expected_version.is_some() {
        return Err(bad("Nová akce zatím nemá verzi."));
    }
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let record: EventRecord = sqlx::query_as("INSERT INTO events(title,description,location,starts_at,ends_at,published,cancelled,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *")
        .bind(value.title).bind(value.description).bind(value.location)
        .bind(value.starts_at).bind(value.ends_at).bind(input.published).bind(input.cancelled)
        .bind(timestamp(now)).fetch_one(&mut *tx).await?;
    audit(&mut tx, Some(admin.id), "created", "event", record.id, now).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(record.into())))
}

pub async fn update(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<EventInput>,
) -> Result<Json<Event>> {
    let value = validate(&input)?;
    let version = expected_version(input.expected_version)?;
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let record: Option<EventRecord> = sqlx::query_as("UPDATE events SET title=$1,description=$2,location=$3,start_time_known=start_time_known OR starts_at IS DISTINCT FROM $4,end_time_known=end_time_known OR ends_at IS DISTINCT FROM $5,end_date_known=end_date_known OR ends_at IS DISTINCT FROM $5,starts_at=$4,ends_at=$5,published=$6,cancelled=$7,updated_at=$8,version=version+1 WHERE id=$9 AND version=$10 RETURNING *")
        .bind(value.title).bind(value.description).bind(value.location)
        .bind(value.starts_at).bind(value.ends_at).bind(input.published).bind(input.cancelled)
        .bind(timestamp(now)).bind(id).bind(version).fetch_optional(&mut *tx).await?;
    let Some(record) = record else {
        return Err(version_mismatch(&mut tx, id).await?);
    };
    audit(&mut tx, Some(admin.id), "updated", "event", id, now).await?;
    tx.commit().await?;
    Ok(Json(record.into()))
}

pub async fn delete(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<EventDelete>,
) -> Result<StatusCode> {
    let version = expected_version(Some(input.expected_version))?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let changed = sqlx::query("DELETE FROM events WHERE id=$1 AND version=$2")
        .bind(id)
        .bind(version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(version_mismatch(&mut tx, id).await?);
    }
    audit(
        &mut tx,
        Some(admin.id),
        "deleted",
        "event",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list(
    State(s): State<Backend>,
    _: Admin,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<Event>>> {
    let (limit, offset) = p.bounds()?;
    let records: Vec<EventRecord> =
        sqlx::query_as("SELECT * FROM events ORDER BY starts_at DESC,id DESC LIMIT $1 OFFSET $2")
            .bind(limit)
            .bind(offset)
            .fetch_all(&s.pool)
            .await?;
    Ok(Json(records.into_iter().map(Into::into).collect()))
}

pub async fn get(State(s): State<Backend>, _: Admin, Path(id): Path<i64>) -> Result<Json<Event>> {
    let record: EventRecord = sqlx::query_as("SELECT * FROM events WHERE id=$1")
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    Ok(Json(record.into()))
}

#[derive(Deserialize, Default)]
pub struct EventQuery {
    #[serde(default)]
    pub archived: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Ongoing events remain upcoming until their end, including cancelled events.
pub async fn public_events_from_pool(
    pool: &PgPool,
    archived: bool,
    limit: i64,
    offset: i64,
    now: OffsetDateTime,
) -> Result<Vec<Event>> {
    let (limit, offset) = Pagination {
        limit: Some(limit),
        offset: Some(offset),
    }
    .bounds()?;
    let query = if archived {
        "SELECT * FROM events WHERE published=TRUE AND ends_at<$1 ORDER BY starts_at DESC,id DESC LIMIT $2 OFFSET $3"
    } else {
        "SELECT * FROM events WHERE published=TRUE AND ends_at>=$1 ORDER BY starts_at,id LIMIT $2 OFFSET $3"
    };
    let records: Vec<EventRecord> = sqlx::query_as(query)
        .bind(now)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(records.into_iter().map(Into::into).collect())
}

pub async fn public_event_from_pool(pool: &PgPool, id: i64) -> Result<Option<Event>> {
    let record: Option<EventRecord> =
        sqlx::query_as("SELECT * FROM events WHERE id=$1 AND published=TRUE")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(record.map(Into::into))
}

pub async fn public_list(
    State(s): State<Backend>,
    Query(q): Query<EventQuery>,
) -> Result<Json<Vec<Event>>> {
    let (limit, offset) = Pagination {
        limit: q.limit,
        offset: q.offset,
    }
    .bounds()?;
    Ok(Json(
        public_events_from_pool(
            &s.pool,
            q.archived,
            limit,
            offset,
            OffsetDateTime::now_utc(),
        )
        .await?,
    ))
}

pub async fn detail(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Json<Event>> {
    Ok(Json(
        public_event_from_pool(&s.pool, id)
            .await?
            .ok_or_else(missing)?,
    ))
}
