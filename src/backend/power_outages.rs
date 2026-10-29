//! Periodic, atomic reconciliation of the public ČEZ town feed into the calendar.
use super::{Backend, Result, audit, auth, cez, timestamp};
use crate::{db::begin_write, outages::OutageSyncStatus};
use axum::{Json, extract::State};
use serde::Serialize;
use time::{Duration, OffsetDateTime};
use time_tz::OffsetDateTimeExt;

const PROVIDER: &str = "cez_distribuce";

/// Separates transport from reconciliation so failure scenarios can be exercised
/// without contacting ČEZ or changing production endpoint configuration.
pub trait OutageSource {
    fn fetch(
        &self,
        town_code: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<cez::Outage>>> + Send;
}

impl OutageSource for cez::Client {
    async fn fetch(&self, town_code: &str) -> anyhow::Result<Vec<cez::Outage>> {
        self.fetch(town_code).await
    }
}

fn local_timestamp(now: OffsetDateTime) -> String {
    timestamp(now.to_timezone(time_tz::timezones::db::europe::PRAGUE))
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct SyncReport {
    pub created: usize,
    pub updated: usize,
    pub withdrawn: usize,
}

#[derive(sqlx::FromRow)]
struct SyncState {
    last_attempt_at: Option<OffsetDateTime>,
    last_success_at: Option<OffsetDateTime>,
    next_attempt_at: OffsetDateTime,
    last_error: Option<String>,
}

pub async fn status(
    State(state): State<Backend>,
    _admin: auth::Admin,
) -> Result<Json<OutageSyncStatus>> {
    let config = &state.config.cez_outages;
    let record: Option<SyncState> = sqlx::query_as(
        "SELECT last_attempt_at,last_success_at,next_attempt_at,last_error
         FROM cez_outage_sync WHERE town_code=$1",
    )
    .bind(&config.town_code)
    .fetch_optional(&state.pool)
    .await?;
    Ok(Json(OutageSyncStatus {
        enabled: config.enabled,
        town_code: config.town_code.clone(),
        interval_seconds: config.interval_seconds,
        last_attempt_at: record
            .as_ref()
            .and_then(|r| r.last_attempt_at)
            .map(local_timestamp),
        last_success_at: record
            .as_ref()
            .and_then(|r| r.last_success_at)
            .map(local_timestamp),
        next_attempt_at: record.as_ref().map(|r| local_timestamp(r.next_attempt_at)),
        last_error: record.and_then(|r| r.last_error),
    }))
}

/// A persisted lease prevents duplicate polling after restart or across replicas.
/// A unique token also prevents a very late response from overwriting a newer run.
async fn claim(state: &Backend, now: OffsetDateTime) -> anyhow::Result<Option<String>> {
    let config = &state.config.cez_outages;
    config.validate()?;
    if !config.enabled {
        return Ok(None);
    }
    let mut tx = begin_write(&state.pool).await?;
    sqlx::query("INSERT INTO cez_outage_sync(town_code,next_attempt_at) VALUES ($1,$2) ON CONFLICT DO NOTHING")
        .bind(&config.town_code).bind(now).execute(&mut *tx).await?;
    let token = auth::token();
    let result = sqlx::query(
        "UPDATE cez_outage_sync SET last_attempt_at=$2,next_attempt_at=$3,run_token=$4
         WHERE town_code=$1 AND next_attempt_at<=$2",
    )
    .bind(&config.town_code)
    .bind(now)
    .bind(now + Duration::seconds(config.interval_seconds as i64))
    .bind(&token)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((result.rows_affected() == 1).then_some(token))
}

pub async fn sync_if_due(
    state: &Backend,
    client: &(impl OutageSource + Sync),
    now: OffsetDateTime,
) -> anyhow::Result<Option<SyncReport>> {
    let Some(token) = claim(state, now).await? else {
        return Ok(None);
    };
    let result = match tokio::time::timeout(
        std::time::Duration::from_secs(60),
        client.fetch(&state.config.cez_outages.town_code),
    )
    .await
    {
        Ok(Ok(snapshot)) => apply_snapshot(state, &token, &snapshot, now).await,
        Ok(Err(error)) => Err(error),
        Err(_) => Err(anyhow::anyhow!(
            "Vypršel čas kontroly odstávek ČEZ Distribuce."
        )),
    };
    match result {
        Ok(report) => Ok(Some(report)),
        Err(error) => {
            // No upstream payload is stored in operational errors.
            let message = error.to_string().chars().take(500).collect::<String>();
            sqlx::query("UPDATE cez_outage_sync SET last_error=$3,run_token=NULL WHERE town_code=$1 AND run_token=$2")
                .bind(&state.config.cez_outages.town_code).bind(&token).bind(message)
                .execute(&state.pool).await?;
            Err(error)
        }
    }
}

async fn apply_snapshot(
    state: &Backend,
    token: &str,
    outages: &[cez::Outage],
    now: OffsetDateTime,
) -> anyhow::Result<SyncReport> {
    let config = &state.config.cez_outages;
    let mut tx = begin_write(&state.pool).await?;
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM cez_outage_sync WHERE town_code=$1 AND run_token=$2 AND next_attempt_at>$3)",
    ).bind(&config.town_code).bind(token).bind(OffsetDateTime::now_utc())
        .fetch_one(&mut *tx).await?;
    anyhow::ensure!(current, "Výsledek kontroly odstávek již není aktuální.");
    let mut report = SyncReport::default();
    let mut seen = Vec::with_capacity(outages.len());
    for outage in outages {
        // The source ID is global, but its municipal address excerpt is scoped.
        let source_id = format!("{}:{}", config.town_code, outage.id);
        seen.push(source_id.clone());
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM events WHERE source_provider=$1 AND source_id=$2")
                .bind(PROVIDER)
                .bind(&source_id)
                .fetch_optional(&mut *tx)
                .await?;
        let event_id = if let Some(id) = existing {
            let result = sqlx::query(
                "UPDATE events SET title=$2,description=$3,location=$4,starts_at=$5,ends_at=$6,
                 source_url=$7,cancelled=$8,published=TRUE,category='other',all_day=FALSE,
                 start_time_known=TRUE,end_time_known=TRUE,end_date_known=TRUE,
                 repeat_every_14_days=FALSE,repeat_until=NULL,version=version+1,updated_at=$9
                 WHERE id=$1 AND ROW(title,description,location,starts_at,ends_at,source_url,cancelled,
                                    published,category,all_day,start_time_known,end_time_known,end_date_known,
                                    repeat_every_14_days,repeat_until)
                 IS DISTINCT FROM ROW($2,$3,$4,$5,$6,$7,$8,TRUE,'other'::TEXT,FALSE,TRUE,TRUE,TRUE,FALSE,NULL::DATE)",
            ).bind(id).bind(&outage.title).bind(&outage.description).bind(&outage.location)
                .bind(outage.starts_at).bind(outage.ends_at).bind(&outage.source_url)
                .bind(outage.cancelled).bind(timestamp(now)).execute(&mut *tx).await?;
            if result.rows_affected() == 1 {
                report.updated += 1;
                audit(&mut tx, None, "cez_sync_update", "event", id, now)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.1))?;
            }
            id
        } else {
            let id = sqlx::query_scalar(
                "INSERT INTO events(title,description,location,starts_at,ends_at,published,cancelled,
                 updated_at,category,all_day,start_time_known,end_time_known,end_date_known,
                 source_provider,source_id,source_url)
                 VALUES ($1,$2,$3,$4,$5,TRUE,$6,$7,'other',FALSE,TRUE,TRUE,TRUE,$8,$9,$10) RETURNING id",
            ).bind(&outage.title).bind(&outage.description).bind(&outage.location)
                .bind(outage.starts_at).bind(outage.ends_at).bind(outage.cancelled)
                .bind(timestamp(now)).bind(PROVIDER).bind(&source_id).bind(&outage.source_url)
                .fetch_one(&mut *tx).await?;
            audit(&mut tx, None, "cez_sync_create", "event", id, now)
                .await
                .map_err(|e| anyhow::anyhow!(e.1))?;
            report.created += 1;
            id
        };
        sqlx::query("INSERT INTO cez_outage_imports(event_id,town_code,last_seen_at) VALUES ($1,$2,$3)
                     ON CONFLICT(event_id) DO UPDATE SET last_seen_at=EXCLUDED.last_seen_at,missing_since=NULL")
            .bind(event_id).bind(&config.town_code).bind(now).execute(&mut *tx).await?;
    }

    // Disappearance is not proof of cancellation. Withdraw only a future event
    // absent from two complete valid snapshots at least one polling interval apart.
    // Ended/ongoing events remain in the archive. Errors never reach this branch.
    let missing: Vec<(i64, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT e.id,i.missing_since FROM cez_outage_imports i JOIN events e ON e.id=i.event_id
         WHERE i.town_code=$1 AND e.source_provider=$2 AND e.published AND NOT e.cancelled AND e.starts_at>$3
         AND NOT (e.source_id=ANY($4))",
    ).bind(&config.town_code).bind(PROVIDER).bind(now.max(OffsetDateTime::now_utc())).bind(&seen)
        .fetch_all(&mut *tx).await?;
    for (id, missing_since) in missing {
        if missing_since
            .is_some_and(|since| now - since >= Duration::seconds(config.interval_seconds as i64))
        {
            sqlx::query(
                "UPDATE events SET published=FALSE,version=version+1,updated_at=$2 WHERE id=$1",
            )
            .bind(id)
            .bind(timestamp(now))
            .execute(&mut *tx)
            .await?;
            audit(&mut tx, None, "cez_sync_withdraw", "event", id, now)
                .await
                .map_err(|e| anyhow::anyhow!(e.1))?;
            report.withdrawn += 1;
        } else if missing_since.is_none() {
            sqlx::query("UPDATE cez_outage_imports SET missing_since=$2 WHERE event_id=$1")
                .bind(id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
    }
    sqlx::query("UPDATE cez_outage_sync SET last_success_at=$2,last_error=NULL,run_token=NULL WHERE town_code=$1")
        .bind(&config.town_code).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(report)
}
