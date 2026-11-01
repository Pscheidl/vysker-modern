//! Rental applications, private attachments and post-rental settlement.
use super::rental_invoice_mail::single_line;
use super::{Backend, Result, audit, auth, bad, conflict, missing, text, timestamp, today};
use axum::{
    Json,
    body::Body,
    extract::{FromRequest, Multipart, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use time::{
    Date, Duration, OffsetDateTime, PrimitiveDateTime, Weekday,
    format_description::well_known::Iso8601,
};
use time_tz::{OffsetResult, PrimitiveDateTimeExt, timezones::db::europe::PRAGUE};

// Revalidate the session and current permission under the same lock as account changes.
pub(super) async fn require_rental_write_access(
    conn: &mut PgConnection,
    admin: &auth::Admin,
) -> Result<()> {
    let permission: Option<bool> = sqlx::query_scalar(
        "SELECT a.can_view_identity FROM administrators a JOIN sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND a.active=TRUE AND s.token_hash=$2 AND s.expires_at>$3",
    )
    .bind(admin.id)
    .bind(&admin.token_hash)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .fetch_optional(conn)
    .await?;
    match permission {
        Some(true) => Ok(()),
        Some(false) => Err(super::Error(
            StatusCode::FORBIDDEN,
            "K této akci nemáte oprávnění.",
        )),
        None => Err(auth::denied()),
    }
}

const FILE_LIMIT: usize = 10 * 1024 * 1024;
const IMAGE_DECODE_LIMIT: u64 = 96 * 1024 * 1024;
const IMAGE_DIMENSION_LIMIT: u32 = 12_000;
const IMAGE_OUTPUT_DIMENSION: u32 = 2560;
static IMAGE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
static INVOICE_UPLOAD_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    hourly_price: Option<i64>,
    // Keep the existing API/storage key, now used for a whole day on any weekday.
    weekend_day_price: Option<i64>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rates {
    #[serde(default)]
    nonprofit: Rate,
    #[serde(default)]
    standard: Rate,
}
impl Rates {
    fn get(&self, event_type: &str) -> &Rate {
        if event_type == "nonprofit" {
            &self.nonprofit
        } else {
            &self.standard
        }
    }
    fn validate(&self) -> Result<()> {
        for rate in [&self.nonprofit, &self.standard] {
            for value in [rate.hourly_price, rate.weekend_day_price]
                .into_iter()
                .flatten()
            {
                if !(0..=10_000_000).contains(&value) {
                    return Err(bad("Neplatná cena v ceníku."));
                }
            }
        }
        Ok(())
    }
}
async fn asset_rates(conn: &mut PgConnection, id: i64) -> Result<Rates> {
    let rows: Vec<(String,Option<i64>,Option<i64>)> = sqlx::query_as("SELECT event_type,hourly_price,weekend_day_price FROM rental_event_rates WHERE asset_id=$1").bind(id).fetch_all(conn).await?;
    let mut rates = Rates::default();
    for (event_type, hourly_price, weekend_day_price) in rows {
        let rate = Rate {
            hourly_price,
            weekend_day_price,
        };
        if event_type == "nonprofit" {
            rates.nonprofit = rate;
        } else {
            rates.standard = rate;
        }
    }
    Ok(rates)
}
async fn save_rates(conn: &mut PgConnection, id: i64, rates: &Rates) -> Result<()> {
    for event_type in ["nonprofit", "standard"] {
        let rate = rates.get(event_type);
        sqlx::query("INSERT INTO rental_event_rates(asset_id,event_type,hourly_price,weekend_day_price) VALUES ($1,$2,$3,$4) ON CONFLICT(asset_id,event_type) DO UPDATE SET hourly_price=EXCLUDED.hourly_price,weekend_day_price=EXCLUDED.weekend_day_price")
            .bind(id).bind(event_type).bind(rate.hourly_price).bind(rate.weekend_day_price).execute(&mut *conn).await?;
    }
    Ok(())
}
fn default_tenant_type() -> String {
    "person".into()
}
fn validate_event(tenant_type: &str, event_type: &str) -> Result<()> {
    if !["person", "organization"].contains(&tenant_type) {
        return Err(bad("Vyberte osobu nebo organizaci."));
    }
    if !["nonprofit", "standard"].contains(&event_type) {
        return Err(bad(
            "Vyberte typ akce. Pokud máte otevřený starší formulář, obnovte stránku.",
        ));
    }
    Ok(())
}
struct TenantDetails {
    address: String,
    organization_ico: Option<String>,
    responsible_name: Option<String>,
}

fn organization_ico(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() != 8 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad("IČO musí obsahovat přesně 8 číslic."));
    }
    // Czech IČO check digit: ČVUT, A. Šolcová, Practical applications of number theory, §2.3.
    // https://portal.cvut.cz/wp-content/uploads/2017/04/HP2008-22-Solcova.pdf
    let digits: Vec<u32> = value.bytes().map(|byte| u32::from(byte - b'0')).collect();
    let sum: u32 = digits[..7]
        .iter()
        .enumerate()
        .map(|(index, digit)| digit * (8 - index as u32))
        .sum();
    if (11 - sum % 11) % 10 != digits[7] {
        return Err(bad(
            "IČO nemá platnou kontrolní číslici. Zkontrolujte jej prosím.",
        ));
    }
    Ok(value.to_owned())
}

fn tenant_details(
    tenant_type: &str,
    address: &str,
    ico: Option<&str>,
    responsible_name: Option<&str>,
) -> Result<TenantDetails> {
    match tenant_type {
        "organization" => Ok(TenantDetails {
            address: String::new(),
            organization_ico: Some(organization_ico(ico.unwrap_or(""))?),
            responsible_name: Some(text(responsible_name.unwrap_or(""), 120)?),
        }),
        "person" => {
            if ico.is_some_and(|v| !v.trim().is_empty())
                || responsible_name.is_some_and(|v| !v.trim().is_empty())
            {
                return Err(bad(
                    "IČO a odpovědnou osobu vyplňte pouze u firmy nebo organizace.",
                ));
            }
            Ok(TenantDetails {
                address: text(address, 300)?,
                organization_ico: None,
                responsible_name: None,
            })
        }
        _ => Err(bad("Vyberte osobu nebo organizaci.")),
    }
}

async fn save_tenant_details(
    conn: &mut PgConnection,
    id: i64,
    details: &TenantDetails,
) -> Result<()> {
    sqlx::query("UPDATE rentals SET organization_ico=$2,responsible_name=$3 WHERE id=$1")
        .bind(id)
        .bind(&details.organization_ico)
        .bind(&details.responsible_name)
        .execute(conn)
        .await?;
    Ok(())
}

async fn tenant_snapshot(
    conn: &mut PgConnection,
    id: i64,
    tenant_type: &str,
    event_type: &str,
    rate: &Rate,
) -> Result<()> {
    sqlx::query("UPDATE rentals SET tenant_type=$2,event_type=$3,rate_group=$3,rate_snapshot=$4 WHERE id=$1")
        .bind(id).bind(tenant_type).bind(event_type).bind(sqlx::types::Json(rate)).execute(conn).await?;
    Ok(())
}

#[derive(Clone, Serialize, sqlx::FromRow)]
pub struct Asset {
    id: i64,
    name: String,
    description: String,
    #[sqlx(skip)]
    rates: Rates,
    notification_email: String,
    require_meter: bool,
    active: bool,
}
#[derive(Deserialize)]
pub struct AssetInput {
    name: String,
    description: String,
    rates: Option<Rates>,
    #[serde(flatten)]
    obsolete: std::collections::HashMap<String, serde_json::Value>,
    notification_email: String,
    require_meter: bool,
    active: bool,
}
#[derive(Serialize, sqlx::FromRow)]
pub struct PublicAsset {
    id: i64,
    name: String,
    description: String,
    #[sqlx(skip)]
    rates: Rates,
    require_meter: bool,
}
#[derive(Serialize, sqlx::FromRow)]
pub struct Interval {
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    status: String,
}
#[derive(Serialize, sqlx::FromRow)]
pub struct Rental {
    id: i64,
    asset_id: i64,
    asset_name: String,
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    invoice_available_at: String,
    invoice_available_unix: i64,
    name: String,
    email: String,
    phone: String,
    address: String,
    organization_ico: Option<String>,
    responsible_name: Option<String>,
    tenant_type: String,
    is_local: bool,
    locality_source: String,
    rate_group: String,
    rate_snapshot: Option<sqlx::types::Json<serde_json::Value>>,
    event_type: Option<String>,
    status: String,
    rental_price: i64,
    source: String,
    billing_error: Option<String>,
    identity_uploaded: bool,
    require_meter: bool,
    wait_for_payment: bool,
    series_id: Option<i64>,
    recurrence_interval_days: Option<i32>,
    recurrence_interval_months: Option<i32>,
    recurrence_until: Option<String>,
    energy_kwh: Option<String>,
    energy_cost: Option<i64>,
    extra_cost: Option<i64>,
    total_price: Option<i64>,
    account_iban: Option<String>,
    account_number: Option<String>,
    bank_code: Option<String>,
    payment_iban: Option<String>,
    invoice_issued_on: Option<String>,
    invoice_due_on: Option<String>,
    invoice_pdf_attached: Option<bool>,
    due_days: Option<i32>,
    invoice_sent_at: Option<String>,
    invoice_mail_status: Option<String>,
    created_at: String,
    before_uploaded: bool,
    after_uploaded: bool,
    before_photo_taken_at: Option<String>,
    after_photo_taken_at: Option<String>,
}
macro_rules! rental_select { ($suffix:literal) => { concat!("SELECT r.id,r.asset_id,a.name AS asset_name,r.start_date::text,r.end_date::text,CASE WHEN r.start_minute IS NOT NULL THEN lpad((r.start_minute/60)::text,2,'0')||':00' END AS start_time,CASE WHEN r.end_minute IS NOT NULL THEN lpad((r.end_minute/60)::text,2,'0')||':00' END AS end_time,to_char(r.ends_local,'YYYY-MM-DD\"T\"HH24:MI:SS') AS invoice_available_at,EXTRACT(EPOCH FROM (r.ends_local AT TIME ZONE 'Europe/Prague'))::bigint AS invoice_available_unix,r.source,r.billing_error,r.identity IS NOT NULL AS identity_uploaded,r.require_meter,r.wait_for_payment,r.series_id,(SELECT interval_days FROM rental_series WHERE id=r.series_id) AS recurrence_interval_days,(SELECT interval_months FROM rental_series WHERE id=r.series_id) AS recurrence_interval_months,(SELECT repeat_until::text FROM rental_series WHERE id=r.series_id) AS recurrence_until,r.name,r.email,r.phone,r.address,r.organization_ico,r.responsible_name,r.tenant_type,r.is_local,r.locality_source,r.rate_group,r.rate_snapshot,r.event_type,r.status,r.rental_price,r.energy_kwh,r.energy_cost,r.extra_cost,r.total_price,r.account_iban,r.account_number,r.bank_code,r.payment_iban,r.invoice_issued_on::text,r.invoice_due_on::text,r.invoice_pdf_attached,r.due_days,r.invoice_sent_at,(SELECT CASE WHEN q.sent_at IS NOT NULL THEN 'sent' WHEN q.attempts>0 AND q.lock_token IS NULL THEN 'retrying' ELSE 'pending' END FROM rental_mail_queue q WHERE q.rental_id=r.id AND q.purpose='invoice' ORDER BY q.id DESC LIMIT 1) AS invoice_mail_status,r.created_at,r.before_photo IS NOT NULL AS before_uploaded,r.after_photo IS NOT NULL AS after_uploaded,r.before_photo_taken_at,r.after_photo_taken_at FROM rentals r JOIN rental_assets a ON a.id=r.asset_id", $suffix) }; }

fn clean_asset(mut a: AssetInput) -> Result<AssetInput> {
    a.name = single_line(&text(&a.name, 120)?);
    a.description = a.description.trim().to_owned();
    if a.description.chars().count() > 5000
        || a.description
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
    {
        return Err(bad("Popis je příliš dlouhý nebo obsahuje neplatné znaky."));
    }
    a.notification_email = auth::email(&a.notification_email)?;
    if let Some(rates) = &a.rates {
        rates.validate()?;
    }
    Ok(a)
}
fn current_rates_input(a: &AssetInput) -> Result<Rates> {
    if !a.obsolete.is_empty() || a.rates.is_none() {
        return Err(bad(
            "Ceník se změnil. Obnovte stránku a vyplňte cenu za hodinu a za celý den pro oba typy akcí.",
        ));
    }
    Ok(a.rates.clone().unwrap())
}
pub async fn assets(State(s): State<Backend>) -> Result<Json<Vec<PublicAsset>>> {
    let mut conn = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut conn).await?;
    let mut assets: Vec<PublicAsset> = sqlx::query_as("SELECT id,name,description,weekday_price,weekend_price,hourly_price,weekend_day_price,require_meter FROM rental_assets WHERE active ORDER BY name,id").fetch_all(&mut *conn).await?;
    for asset in &mut assets {
        asset.rates = asset_rates(&mut conn, asset.id).await?;
    }
    Ok(Json(assets))
}
pub async fn admin_assets(State(s): State<Backend>, _: auth::Admin) -> Result<Json<Vec<Asset>>> {
    let mut conn = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut conn).await?;
    let mut assets: Vec<Asset> = sqlx::query_as("SELECT id,name,description,weekday_price,weekend_price,hourly_price,weekend_day_price,notification_email,require_meter,active FROM rental_assets ORDER BY name,id").fetch_all(&mut *conn).await?;
    for asset in &mut assets {
        asset.rates = asset_rates(&mut conn, asset.id).await?;
    }
    Ok(Json(assets))
}
pub async fn create_asset(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(a): Json<AssetInput>,
) -> Result<Json<Asset>> {
    let a = clean_asset(a)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let rates = current_rates_input(&a)?;
    let mut row: Asset=sqlx::query_as("INSERT INTO rental_assets(name,description,weekday_price,weekend_price,notification_email,require_meter,active) VALUES ($1,$2,0,0,$3,$4,$5) RETURNING id,name,description,notification_email,require_meter,active")
        .bind(a.name).bind(a.description).bind(a.notification_email).bind(a.require_meter).bind(a.active).fetch_one(&mut *tx).await?;
    save_rates(&mut tx, row.id, &rates).await?;
    row.rates = rates;
    audit(
        &mut tx,
        Some(admin.id),
        "created",
        "rental_asset",
        row.id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(row))
}
pub async fn update_asset(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(a): Json<AssetInput>,
) -> Result<Json<Asset>> {
    let a = clean_asset(a)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let rates = current_rates_input(&a)?;
    let mut row: Asset=sqlx::query_as("UPDATE rental_assets SET name=$1,description=$2,notification_email=$3,require_meter=$4,active=$5 WHERE id=$6 RETURNING id,name,description,notification_email,require_meter,active")
        .bind(a.name).bind(a.description).bind(a.notification_email).bind(a.require_meter).bind(a.active).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(missing)?;
    save_rates(&mut tx, id, &rates).await?;
    row.rates = rates;
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "rental_asset",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(row))
}
pub async fn availability(
    State(s): State<Backend>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Interval>>> {
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rental_assets WHERE id=$1 AND active)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(missing());
    }
    let now = OffsetDateTime::now_utc();
    let day = today(now);
    Ok(Json(sqlx::query_as("SELECT start_date::text,end_date::text,CASE WHEN start_minute IS NOT NULL THEN lpad((start_minute/60)::text,2,'0')||':00' END AS start_time,CASE WHEN end_minute IS NOT NULL THEN lpad((end_minute/60)::text,2,'0')||':00' END AS end_time,status FROM (SELECT start_date,end_date,start_minute,end_minute,CASE WHEN status='pending' THEN 'pending' ELSE 'booked' END AS status FROM rentals WHERE asset_id=$1 AND status NOT IN ('rejected','cancelled') AND (status<>'pending' OR (starts_local AT TIME ZONE 'Europe/Prague')>$3) AND end_date >= $2 UNION ALL SELECT start_date,end_date,start_minute,end_minute,'booked' AS status FROM rental_blocks WHERE asset_id=$1 AND end_date >= $2) AS periods ORDER BY start_date,end_date,status")
        .bind(id).bind(day).bind(now).fetch_all(&mut *tx).await?))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct RentalBlock {
    id: i64,
    asset_id: i64,
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockInput {
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
}

pub async fn list_blocks(
    State(s): State<Backend>,
    _: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<Vec<RentalBlock>>> {
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rental_assets WHERE id=$1)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(missing());
    }
    Ok(Json(sqlx::query_as("SELECT id,asset_id,start_date::text,end_date::text,CASE WHEN start_minute IS NOT NULL THEN lpad((start_minute/60)::text,2,'0')||':00' END AS start_time,CASE WHEN end_minute IS NOT NULL THEN lpad((end_minute/60)::text,2,'0')||':00' END AS end_time FROM rental_blocks WHERE asset_id=$1 ORDER BY start_date,end_date,id").bind(id).fetch_all(&mut *tx).await?))
}

pub async fn create_block(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<BlockInput>,
) -> Result<(StatusCode, Json<RentalBlock>)> {
    let period = Period::new(
        &input.start_date,
        &input.end_date,
        input.start_time.as_deref(),
        input.end_time.as_deref(),
        false,
    )?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    // Share the asset row lock with public requests so neither can race the other.
    let asset: Option<i64> =
        sqlx::query_scalar("SELECT id FROM rental_assets WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if asset.is_none() {
        return Err(missing());
    }
    expire_pending(&mut tx, OffsetDateTime::now_utc(), Some(id), None).await?;
    ensure_available(&mut tx, id, &period).await?;
    let block_id: i64 = sqlx::query_scalar("INSERT INTO rental_blocks(asset_id,start_date,end_date,start_minute,end_minute) VALUES ($1,$2,$3,$4,$5) RETURNING id")
        .bind(id).bind(period.start).bind(period.end).bind(period.start_minute).bind(period.end_minute).fetch_one(&mut *tx).await?;
    let block = RentalBlock {
        id: block_id,
        asset_id: id,
        start_date: period.start.to_string(),
        end_date: period.end.to_string(),
        start_time: period.start_time(),
        end_time: period.end_time(),
    };
    audit(
        &mut tx,
        Some(admin.id),
        "created",
        "rental_block",
        block.id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(block)))
}

pub async fn delete_block(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path((id, block_id)): Path<(i64, i64)>,
) -> Result<StatusCode> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM rental_assets WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(missing());
    }
    let changed = sqlx::query("DELETE FROM rental_blocks WHERE id=$1 AND asset_id=$2")
        .bind(block_id)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(missing());
    }
    audit(
        &mut tx,
        Some(admin.id),
        "deleted",
        "rental_block",
        block_id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
fn date(value: &str) -> Result<Date> {
    let parsed = Date::parse(value, &Iso8601::DEFAULT).map_err(|_| bad("Neplatné datum."))?;
    if !(2000..=9998).contains(&parsed.year()) {
        return Err(bad("Neplatné datum."));
    }
    Ok(parsed)
}

#[derive(Clone, Copy)]
struct Period {
    start: Date,
    end: Date,
    start_minute: Option<i16>,
    end_minute: Option<i16>,
}
impl Period {
    fn new(
        start: &str,
        end: &str,
        from: Option<&str>,
        to: Option<&str>,
        rental: bool,
    ) -> Result<Self> {
        let start = date(start)?;
        let end = date(end)?;
        if end < start || (rental && (end - start).whole_days() >= 28) {
            return Err(bad(
                "Pronájem může trvat nejvýše 28 po sobě jdoucích dnů a konec nesmí předcházet začátku.",
            ));
        }
        let parse_hour = |value: &str, end: bool| -> Result<i16> {
            if value.len() != 5 || !value.is_ascii() || !value.ends_with(":00") {
                return Err(bad("Zadejte čas v celých hodinách, například 18:00."));
            }
            let hour = value[..2]
                .parse::<i16>()
                .map_err(|_| bad("Neplatný čas."))?;
            if !(0..=if end { 24 } else { 23 }).contains(&hour) {
                return Err(bad("Neplatný čas."));
            }
            Ok(hour * 60)
        };
        let from = from.filter(|v| !v.is_empty());
        let to = to.filter(|v| !v.is_empty());
        let (start_minute, end_minute) = match (from, to) {
            (None, None) => (None, None),
            (Some(from), Some(to)) => {
                let first = parse_hour(from, false)?;
                let last = parse_hour(to, true)?;
                if start != end || last <= first {
                    return Err(bad(
                        "Hodinová rezervace musí začínat i končit ve stejný den a mít kladnou délku.",
                    ));
                }
                if rental && matches!(start.weekday(), Weekday::Saturday | Weekday::Sunday) {
                    return Err(bad(
                        "Sobotu a neděli lze rezervovat pouze jako celé dny. Hodinové rezervace jsou možné od pondělí do pátku.",
                    ));
                }
                (Some(first), Some(last))
            }
            _ => return Err(bad("Vyplňte začátek i konec hodinové rezervace.")),
        };
        let period = Self {
            start,
            end,
            start_minute,
            end_minute,
        };
        // A repeated/missing wall-clock hour cannot be sold as an ordinary one-hour slot.
        let first = period.start_instant()?;
        let last = period.end_instant()?;
        if let (Some(a), Some(b)) = (start_minute, end_minute) {
            if (last - first).whole_minutes() != i64::from(b - a) {
                return Err(bad(
                    "Tento čas zasahuje do změny letního času. Zvolte jiný hodinový úsek.",
                ));
            }
        }
        Ok(period)
    }
    fn local_start(self) -> PrimitiveDateTime {
        self.start.midnight() + Duration::minutes(i64::from(self.start_minute.unwrap_or(0)))
    }
    fn local_end(self) -> PrimitiveDateTime {
        self.end.midnight() + Duration::minutes(i64::from(self.end_minute.unwrap_or(1440)))
    }
    fn instant(local: PrimitiveDateTime) -> Result<OffsetDateTime> {
        match local.assume_timezone(PRAGUE) {
            OffsetResult::Some(v) => Ok(v),
            _ => Err(bad(
                "Zvolený čas je při změně letního času nejednoznačný nebo neexistuje. Zvolte jiný čas.",
            )),
        }
    }
    fn start_instant(self) -> Result<OffsetDateTime> {
        Self::instant(self.local_start())
    }
    fn has_started(self, now: OffsetDateTime) -> Result<bool> {
        Ok(self.start_instant()? <= now)
    }
    fn require_future_start(self, now: OffsetDateTime) -> Result<()> {
        if self.has_started(now)? {
            return Err(bad(
                "Začátek pronájmu musí být v budoucnosti. Zvolte pozdější termín.",
            ));
        }
        Ok(())
    }
    fn end_instant(self) -> Result<OffsetDateTime> {
        Self::instant(self.local_end())
    }
    fn start_time(self) -> Option<String> {
        self.start_minute.map(|n| format!("{:02}:00", n / 60))
    }
    fn end_time(self) -> Option<String> {
        self.end_minute.map(|n| format!("{:02}:00", n / 60))
    }
    fn from_rental(r: &Rental) -> Result<Self> {
        Self::new(
            &r.start_date,
            &r.end_date,
            r.start_time.as_deref(),
            r.end_time.as_deref(),
            false,
        )
    }
    fn price(self, rate: &Rate) -> Result<i64> {
        if let (Some(a), Some(b)) = (self.start_minute, self.end_minute) {
            let hourly = rate.hourly_price.ok_or_else(|| {
                bad("Pro tento typ akce není nastavena hodinová cena. Kontaktujte obecní úřad.")
            })?;
            return hourly
                .checked_mul(i64::from((b - a) / 60))
                .ok_or_else(|| bad("Neplatná cena."));
        }
        let daily = rate.weekend_day_price.ok_or_else(|| {
            bad("Pro tento typ akce není nastavena cena za celý den. Kontaktujte obecní úřad.")
        })?;
        daily
            .checked_mul((self.end - self.start).whole_days() + 1)
            .ok_or_else(|| bad("Neplatná cena."))
    }
    fn label(self) -> String {
        if let (Some(a), Some(b)) = (self.start_time(), self.end_time()) {
            format!("{} od {} do {}", self.start, a, b)
        } else {
            format!("{} až {}", self.start, self.end)
        }
    }
}

async fn ensure_available(conn: &mut PgConnection, asset_id: i64, period: &Period) -> Result<()> {
    let occupied: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rentals WHERE asset_id=$1 AND status NOT IN ('rejected','cancelled') AND (status<>'pending' OR (starts_local AT TIME ZONE 'Europe/Prague')>$4) AND starts_local<$3 AND ends_local>$2)")
        .bind(asset_id).bind(period.local_start()).bind(period.local_end()).bind(OffsetDateTime::now_utc()).fetch_one(&mut *conn).await?;
    if occupied {
        return Err(conflict(
            "Vybraný termín se překrývá s jinou žádostí nebo rezervací.",
        ));
    }
    let blocked: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rental_blocks WHERE asset_id=$1 AND starts_local<$3 AND ends_local>$2)")
        .bind(asset_id).bind(period.local_start()).bind(period.local_end()).fetch_one(&mut *conn).await?;
    if blocked {
        return Err(conflict("Vybraný termín je blokovaný obcí."));
    }
    Ok(())
}
fn check_image(bytes: &[u8]) -> Result<&'static str> {
    if bytes.is_empty() || bytes.len() > FILE_LIMIT {
        return Err(bad("Soubor může mít nejvýše 10 MB."));
    }
    let f = image::guess_format(bytes)
        .map_err(|_| bad("Soubor musí být obrázek JPEG, PNG nebo WebP."))?;
    let mime = match f {
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::WebP => "image/webp",
        _ => return Err(bad("Soubor musí být obrázek JPEG, PNG nebo WebP.")),
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), f);
    reader.limits(image_limits());
    let decoder = reader.into_decoder().map_err(image_read_error)?;
    check_decoded_image_limits(&decoder)?;
    Ok(mime)
}

fn image_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(IMAGE_DIMENSION_LIMIT);
    limits.max_image_height = Some(IMAGE_DIMENSION_LIMIT);
    limits.max_alloc = Some(IMAGE_DECODE_LIMIT);
    limits
}

fn image_read_error(error: image::ImageError) -> super::Error {
    if matches!(error, image::ImageError::Limits(_)) {
        bad("Fotografie je příliš velká pro zpracování. Snižte její rozlišení a nahrajte ji znovu.")
    } else {
        bad("Fotografii nelze přečíst.")
    }
}

fn check_decoded_image_limits(decoder: &impl image::ImageDecoder) -> Result<()> {
    let (width, height) = decoder.dimensions();
    // into_decoder + DynamicImage::from_decoder does not reserve the output
    // buffer against Limits. Enforce this independently of codec support.
    if width == 0
        || height == 0
        || width > IMAGE_DIMENSION_LIMIT
        || height > IMAGE_DIMENSION_LIMIT
        || u64::from(width) * u64::from(height) > 60_000_000
        || decoder.total_bytes() > IMAGE_DECODE_LIMIT
    {
        return Err(bad(
            "Fotografie je příliš velká pro zpracování. Snižte její rozlišení a nahrajte ji znovu.",
        ));
    }
    Ok(())
}

async fn process_image_files<T: Send + 'static>(
    slot: tokio::sync::SemaphorePermit<'static>,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || {
        // A disconnected request must not release the slot while its blocking
        // decode still runs. The worker owns the permit until all buffers drop.
        let _slot = slot;
        work()
    })
    .await
    .map_err(|_| bad("Fotografii nelze zpracovat."))?
}
// The camera clock is an untrusted, timezone-free hint, never proof of timing.
// Read only DateTimeOriginal. No GPS or other metadata is retained.
fn photo_taken_at(bytes: &[u8]) -> Option<String> {
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(bytes))
        .ok()?;
    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    let exif::Value::Ascii(values) = &field.value else {
        return None;
    };
    let raw = std::str::from_utf8(values.first()?).ok()?;
    let b = raw.as_bytes();
    if b.len() != 19
        || b[4] != b':'
        || b[7] != b':'
        || b[10] != b' '
        || b[13] != b':'
        || b[16] != b':'
        || b.iter()
            .enumerate()
            .any(|(i, c)| ![4, 7, 10, 13, 16].contains(&i) && !c.is_ascii_digit())
    {
        return None;
    }
    let mut normalized = raw.to_owned();
    normalized.replace_range(4..5, "-");
    normalized.replace_range(7..8, "-");
    let format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    time::PrimitiveDateTime::parse(&normalized, format).ok()?;
    Some(normalized)
}
fn sanitize_meter_photo(bytes: &[u8]) -> Result<Vec<u8>> {
    let format = image::guess_format(bytes).map_err(|_| bad("Neplatná fotografie."))?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.limits(image_limits());
    let decoder = reader.into_decoder().map_err(image_read_error)?;
    sanitize_decoded_photo(decoder)
}

fn sanitize_decoded_photo(mut decoder: impl image::ImageDecoder) -> Result<Vec<u8>> {
    check_decoded_image_limits(&decoder)?;
    let orientation = decoder
        .orientation()
        .map_err(|_| bad("Fotografii nelze přečíst."))?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(image_read_error)?;
    // Thumbnail uses an output-sized buffer. Resize before orientation, which
    // can otherwise allocate a second full-resolution image for a 90° turn.
    if image.width() > IMAGE_OUTPUT_DIMENSION || image.height() > IMAGE_OUTPUT_DIMENSION {
        image = image.thumbnail(IMAGE_OUTPUT_DIMENSION, IMAGE_OUTPUT_DIMENSION);
    }
    image.apply_orientation(orientation);
    let mut output = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 90)
        .encode_image(&image)
        .map_err(|_| bad("Fotografii nelze zpracovat."))?;
    Ok(output)
}
async fn queue(
    conn: &mut PgConnection,
    id: i64,
    to: &str,
    subject: &str,
    body: &str,
    purpose: &str,
    now: i64,
) -> Result<i64> {
    let queue_id = sqlx::query_scalar("INSERT INTO rental_mail_queue(rental_id,recipient,subject,body,purpose,next_attempt_at,created_at) VALUES ($1,$2,$3,$4,$5,$6,$6) RETURNING id")
    .bind(id).bind(to).bind(subject).bind(body).bind(purpose).bind(now).fetch_one(conn).await?;
    Ok(queue_id)
}

async fn cancel_application_messages(conn: &mut PgConnection, id: i64, now: i64) -> Result<()> {
    // A decision supersedes both the applicant's receipt and the office notification.
    // Keep the same rental -> queue lock order as delivery.
    sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$2,body=NULL,lock_token=NULL,locked_until=0 WHERE rental_id=$1 AND purpose='application' AND sent_at IS NULL AND cancelled_at IS NULL")
        .bind(id).bind(now).execute(conn).await?;
    Ok(())
}

// Call under the existing rental write locks, or after locking one rental for delivery.
// The conditional state transition makes retries and concurrent maintenance idempotent.
async fn expire_pending(
    conn: &mut PgConnection,
    now: OffsetDateTime,
    asset_id: Option<i64>,
    rental_id: Option<i64>,
) -> Result<usize> {
    let expired: Vec<Rental> = sqlx::query_as(rental_select!(" WHERE r.status='pending' AND (r.starts_local AT TIME ZONE 'Europe/Prague')<=$1 AND ($2::BIGINT IS NULL OR r.asset_id=$2) AND ($3::BIGINT IS NULL OR r.id=$3) ORDER BY r.id FOR UPDATE OF r LIMIT 100"))
        .bind(now).bind(asset_id).bind(rental_id).fetch_all(&mut *conn).await?;
    let count = expired.len();
    for rental in expired {
        let changed = sqlx::query("UPDATE rentals SET status='cancelled',identity=NULL,identity_type=NULL WHERE id=$1 AND status='pending' AND (starts_local AT TIME ZONE 'Europe/Prague')<=$2")
            .bind(rental.id).bind(now).execute(&mut *conn).await?.rows_affected();
        if changed == 0 {
            continue;
        }
        sqlx::query("DELETE FROM rental_access_tokens WHERE rental_id=$1")
            .bind(rental.id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$2,body=NULL,invoice_attachment=NULL,lock_token=NULL,locked_until=0 WHERE rental_id=$1 AND sent_at IS NULL AND cancelled_at IS NULL")
            .bind(rental.id).bind(now.unix_timestamp()).execute(&mut *conn).await?;
        queue(conn, rental.id, &rental.email, "Žádost o pronájem byla automaticky zrušena", &format!("Dobrý den,\n\nVaše žádost č. {} o pronájem {} v termínu {} nebyla potvrzena do začátku pronájmu. Systém ji proto automaticky zrušil a termín uvolnil. Omlouváme se.\n\nV případě dotazů prosím kontaktujte obecní úřad.\n\nObec Vyskeř", rental.id, single_line(&rental.asset_name), Period::from_rental(&rental)?.label()), "expired_unapproved", now.unix_timestamp()).await?;
        audit(conn, None, "expired_unapproved", "rental", rental.id, now).await?;
    }
    Ok(count)
}

#[derive(Serialize)]
pub struct Created {
    id: i64,
}
pub async fn create(
    State(s): State<Backend>,
    headers: HeaderMap,
    ip: auth::ClientIp,
    mut form: Multipart,
) -> Result<(StatusCode, Json<Created>)> {
    auth::check_origin(&headers, &s)?;
    let now = OffsetDateTime::now_utc();
    auth::throttle(&s, &format!("rental:{}", ip.0), 10, 3600, now).await?;
    let image_slot =
        tokio::time::timeout(std::time::Duration::from_secs(60), IMAGE_SLOTS.acquire())
            .await
            .map_err(|_| bad("Zpracování souborů je zaneprázdněné, zkuste to znovu."))?
            .map_err(|_| bad("Soubor nelze zpracovat."))?;
    let (fields, identity) = tokio::time::timeout(std::time::Duration::from_secs(60), async move {
        let mut fields = std::collections::HashMap::new();
        let mut identity = None;
        while let Some(field) = form
            .next_field()
            .await
            .map_err(|_| bad("Neplatný formulář."))?
        {
            let name = field.name().unwrap_or("").to_string();
            if name == "identity" {
                if identity.is_some() {
                    return Err(bad("Nahrajte jednu kopii dokladu."));
                }
                identity = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|_| bad("Soubor se nepodařilo načíst."))?
                        .to_vec(),
                );
            } else if [
                "asset_id",
                "start_date",
                "end_date",
                "start_time",
                "end_time",
                "name",
                "email",
                "phone",
                "address",
                "tenant_type",
                "organization_ico",
                "responsible_name",
                "event_type",
                "consent",
            ]
            .contains(&name.as_str())
            {
                if fields.contains_key(&name) {
                    return Err(bad("Pole je zadáno vícekrát."));
                }
                let val = field.text().await.map_err(|_| bad("Neplatný formulář."))?;
                if val.len() > 10_000 {
                    return Err(bad("Pole je příliš dlouhé."));
                }
                fields.insert(name, val);
            } else {
                return Err(bad("Neznámé pole formuláře."));
            }
        }
        Ok::<_, super::Error>((fields, identity))
    })
    .await
    .map_err(|_| bad("Nahrávání trvalo příliš dlouho, zkuste to znovu."))??;
    let get = |k: &str| fields.get(k).map(String::as_str).unwrap_or("");
    if get("consent") != "true" {
        return Err(bad(
            "Je nutný výslovný souhlas s pořízením a uložením kopie dokladu.",
        ));
    }
    let id: i64 = get("asset_id")
        .parse()
        .map_err(|_| bad("Vyberte předmět pronájmu."))?;
    let period = Period::new(
        get("start_date"),
        get("end_date"),
        Some(get("start_time")),
        Some(get("end_time")),
        true,
    )?;
    period.require_future_start(OffsetDateTime::now_utc())?;
    let name = text(get("name"), 120)?;
    let phone = text(get("phone"), 40)?;
    if phone.chars().filter(char::is_ascii_digit).count() < 6
        || !phone
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_whitespace() || "+()-./".contains(c))
    {
        return Err(bad("Zadejte platné telefonní číslo."));
    }
    let email = auth::email(get("email"))?;
    let tenant_type = if get("tenant_type").is_empty() {
        "person"
    } else {
        get("tenant_type")
    };
    let event_type = get("event_type");
    validate_event(tenant_type, event_type)?;
    let details = tenant_details(
        tenant_type,
        get("address"),
        fields.get("organization_ico").map(String::as_str),
        fields.get("responsible_name").map(String::as_str),
    )?;
    let identity =
        identity.ok_or_else(|| bad("Nahrajte kopii dokladu žadatele nebo odpovědné osoby."))?;
    let identity = process_image_files(image_slot, move || {
        check_image(&identity)?;
        sanitize_meter_photo(&identity)
    })
    .await?;
    let mime = "image/jpeg";
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let a:Asset=sqlx::query_as("SELECT id,name,description,weekday_price,weekend_price,hourly_price,weekend_day_price,notification_email,require_meter,active FROM rental_assets WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(missing)?;
    if !a.active {
        return Err(missing());
    }
    let now = OffsetDateTime::now_utc();
    period.require_future_start(now)?;
    let rates = asset_rates(&mut tx, id).await?;
    let rate = rates.get(event_type);
    let rental_price = period.price(rate)?;
    expire_pending(&mut tx, OffsetDateTime::now_utc(), Some(id), None).await?;
    ensure_available(&mut tx, id, &period).await?;
    // Waiting for expired rentals or a delivery lock must not admit a request after its deadline.
    let now = OffsetDateTime::now_utc();
    period.require_future_start(now)?;
    let rental_id:i64=sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,start_minute,end_minute,name,email,phone,address,rental_price,created_at,identity,identity_type,require_meter,identity_consent_at) VALUES ($1,$2,$3,$13,$14,$4,$5,$6,$7,$8,$9,$10,$11,$12,$9) RETURNING id")
        .bind(id).bind(period.start).bind(period.end).bind(&name).bind(&email).bind(phone).bind(&details.address).bind(rental_price).bind(timestamp(now)).bind(identity).bind(mime).bind(a.require_meter).bind(period.start_minute).bind(period.end_minute).fetch_one(&mut *tx).await?;
    tenant_snapshot(&mut tx, rental_id, tenant_type, event_type, rate).await?;
    save_tenant_details(&mut tx, rental_id, &details).await?;
    let energy_note = if a.require_meter {
        " Energie a případné další náklady budou vyúčtovány po skončení pronájmu."
    } else {
        " Vyúčtování přijde po skončení pronájmu."
    };
    queue(&mut tx,rental_id,&email,"Žádost o pronájem přijata",&format!("Přijali jsme vaši žádost o pronájem {} v termínu {}. Žádost čeká na schválení.\nCena nájmu: {} Kč.{}",single_line(&a.name),period.label(),rental_price,energy_note),"application",now.unix_timestamp()).await?;
    queue(&mut tx,rental_id,&a.notification_email,"Nová žádost o pronájem",&format!("Byla přijata nová žádost č. {} o pronájem {} v termínu {}. Podrobnosti najdete v administraci.",rental_id,single_line(&a.name),period.label()),"application",now.unix_timestamp()).await?;
    // Roll back every write if processing has crossed the start of the selected term.
    period.require_future_start(OffsetDateTime::now_utc())?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(Created { id: rental_id })))
}
pub async fn admin_list(State(s): State<Backend>, _: auth::Admin) -> Result<Json<Vec<Rental>>> {
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    Ok(Json(
        sqlx::query_as(rental_select!(" ORDER BY r.id DESC"))
            .fetch_all(&mut *tx)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRentalInput {
    asset_id: i64,
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    name: String,
    email: String,
    phone: String,
    #[serde(default)]
    address: String,
    organization_ico: Option<String>,
    responsible_name: Option<String>,
    #[serde(default)]
    consent: bool,
    #[serde(default = "default_tenant_type")]
    tenant_type: String,
    event_type: String,
    rental_price: i64,
    require_meter: bool,
    wait_for_payment: bool,
    #[serde(default)]
    recurrence_interval_days: i32,
    #[serde(default)]
    recurrence_interval_months: i32,
    recurrence_until: Option<String>,
}

impl AdminRentalInput {
    fn recurring(&self) -> bool {
        self.recurrence_interval_days != 0 || self.recurrence_interval_months != 0
    }

    fn periods(&self, now: OffsetDateTime) -> Result<Vec<Period>> {
        validate_event(&self.tenant_type, &self.event_type)?;
        if !(0..=100_000_000).contains(&self.rental_price) {
            return Err(bad("Neplatná cena za jeden pronájem."));
        }
        if ![0, 7, 14].contains(&self.recurrence_interval_days)
            || ![0, 1].contains(&self.recurrence_interval_months)
            || (self.recurrence_interval_days != 0 && self.recurrence_interval_months != 0)
        {
            return Err(bad(
                "Zvolte jeden způsob opakování: každý týden, každých 14 dní nebo jednou měsíčně.",
            ));
        }
        let first = Period::new(
            &self.start_date,
            &self.end_date,
            self.start_time.as_deref(),
            self.end_time.as_deref(),
            true,
        )?;
        first.require_future_start(now)?;
        let until = if !self.recurring() {
            if self
                .recurrence_until
                .as_deref()
                .is_some_and(|s| !s.is_empty())
            {
                return Err(bad("Konec opakování vyplňte pouze u opakované rezervace."));
            }
            first.start
        } else {
            let last = date(
                self.recurrence_until
                    .as_deref()
                    .ok_or_else(|| bad("Vyberte datum konce opakování."))?,
            )?;
            if last < first.start || (last - first.start).whole_days() > 366 {
                return Err(bad("Opakování lze připravit nejvýše na jeden rok."));
            }
            last
        };
        let starts = if self.recurrence_interval_months == 1 {
            monthly_weekday_starts(first.start, until)?
        } else {
            let mut starts = vec![first.start];
            if self.recurrence_interval_days > 0 {
                let step = Duration::days(i64::from(self.recurrence_interval_days));
                let mut next = first.start.checked_add(step);
                while let Some(start) = next.filter(|start| *start <= until) {
                    starts.push(start);
                    next = start.checked_add(step);
                }
            }
            starts
        };
        let mut output: Vec<Period> = Vec::new();
        for start in starts {
            let end = start
                .checked_add(first.end - first.start)
                .ok_or_else(|| bad("Termín opakování je mimo podporovaný rozsah dat."))?;
            let period = Period::new(
                &start.to_string(),
                &end.to_string(),
                self.start_time.as_deref(),
                self.end_time.as_deref(),
                true,
            )?;
            if output
                .last()
                .is_some_and(|previous| previous.local_end() > period.local_start())
            {
                return Err(bad(
                    "Opakované termíny se nesmějí překrývat. Zkraťte pronájem nebo prodlužte interval opakování.",
                ));
            }
            output.push(period);
        }
        Ok(output)
    }
    fn contact(&self) -> Result<(String, String, String, TenantDetails)> {
        let phone = text(&self.phone, 40)?;
        if phone.chars().filter(char::is_ascii_digit).count() < 6
            || !phone
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_whitespace() || "+()-./".contains(c))
        {
            return Err(bad("Zadejte platné telefonní číslo."));
        }
        Ok((
            text(&self.name, 120)?,
            auth::email(&self.email)?,
            phone,
            tenant_details(
                &self.tenant_type,
                &self.address,
                self.organization_ico.as_deref(),
                self.responsible_name.as_deref(),
            )?,
        ))
    }
}

// Calendar-month recurrence is anchored to the original ordinal weekday.
// Missing fifth weekdays are omitted rather than shifted to the fourth.
fn monthly_weekday_starts(first: Date, until: Date) -> Result<Vec<Date>> {
    let first_month = first.year() * 12 + i32::from(first.month() as u8) - 1;
    let last_month = until.year() * 12 + i32::from(until.month() as u8) - 1;
    let week = (first.day() - 1) / 7;
    let weekday = first.weekday().number_days_from_monday();
    let mut starts = Vec::new();
    for index in first_month..=last_month {
        let month = time::Month::try_from((index.rem_euclid(12) + 1) as u8)
            .map_err(|_| bad("Neplatný měsíc opakování."))?;
        let month_start = Date::from_calendar_date(index.div_euclid(12), month, 1)
            .map_err(|_| bad("Termín opakování je mimo podporovaný rozsah dat."))?;
        let day =
            1 + (weekday + 7 - month_start.weekday().number_days_from_monday()) % 7 + 7 * week;
        if let Ok(start) = month_start.replace_day(day) {
            if start >= first && start <= until {
                starts.push(start);
            }
        }
    }
    Ok(starts)
}

#[derive(Serialize)]
pub struct OccurrencePreview {
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    conflict: bool,
}
#[derive(Serialize)]
pub struct RentalPreview {
    occurrences: Vec<OccurrencePreview>,
    conflict_count: usize,
    occurrence_count: usize,
    total_price: i64,
}
pub async fn preview_admin_rental(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<AdminRentalInput>,
) -> Result<Json<RentalPreview>> {
    let periods = input.periods(OffsetDateTime::now_utc())?;
    if input.tenant_type == "organization" {
        tenant_details(
            &input.tenant_type,
            &input.address,
            input.organization_ico.as_deref(),
            input.responsible_name.as_deref(),
        )?;
    }
    let mut conn = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut conn).await?;
    require_rental_write_access(&mut conn, &admin).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rental_assets WHERE id=$1)")
        .bind(input.asset_id)
        .fetch_one(&mut *conn)
        .await?;
    if !exists {
        return Err(missing());
    }
    let mut occurrences = Vec::with_capacity(periods.len());
    for (index, p) in periods.iter().enumerate() {
        let external = match ensure_available(&mut conn, input.asset_id, p).await {
            Ok(()) => false,
            Err(error) if error.0 == StatusCode::CONFLICT => true,
            Err(error) => return Err(error),
        };
        let own = periods.iter().enumerate().any(|(other, q)| {
            other != index && p.local_start() < q.local_end() && q.local_start() < p.local_end()
        });
        occurrences.push(OccurrencePreview {
            start_date: p.start.to_string(),
            end_date: p.end.to_string(),
            start_time: p.start_time(),
            end_time: p.end_time(),
            conflict: external || own,
        });
    }
    // Availability reads can wait too, so never return a preview whose start has passed.
    let now = OffsetDateTime::now_utc();
    for period in &periods {
        period.require_future_start(now)?;
    }
    Ok(Json(RentalPreview {
        conflict_count: occurrences.iter().filter(|p| p.conflict).count(),
        occurrence_count: occurrences.len(),
        total_price: input.rental_price * (occurrences.len() as i64),
        occurrences,
    }))
}

#[derive(Serialize)]
pub struct AdminCreated {
    series_id: Option<i64>,
    rentals: Vec<Rental>,
}
async fn admin_rental_form(
    s: &Backend,
    request: Request,
) -> Result<(AdminRentalInput, Option<Vec<u8>>)> {
    let multipart = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"));
    if !multipart {
        let Json(input) = Json::<AdminRentalInput>::from_request(request, s)
            .await
            .map_err(|error| super::Error(error.status(), "Neplatné údaje rezervace."))?;
        return Ok((input, None));
    }
    let image_slot =
        tokio::time::timeout(std::time::Duration::from_secs(60), IMAGE_SLOTS.acquire())
            .await
            .map_err(|_| bad("Zpracování souborů je zaneprázdněné, zkuste to znovu."))?
            .map_err(|_| bad("Soubor nelze zpracovat."))?;
    let mut form = Multipart::from_request(request, s)
        .await
        .map_err(|_| bad("Neplatný formulář."))?;
    let (input, identity) = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut input = None;
        let mut identity = None;
        while let Some(field) = form
            .next_field()
            .await
            .map_err(|_| bad("Neplatný formulář."))?
        {
            match field.name().unwrap_or("") {
                "data" if input.is_none() => {
                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|_| bad("Neplatné údaje rezervace."))?;
                    if bytes.len() > 64 * 1024 {
                        return Err(bad("Údaje rezervace jsou příliš dlouhé."));
                    }
                    input = Some(
                        serde_json::from_slice::<AdminRentalInput>(&bytes)
                            .map_err(|_| bad("Neplatné údaje rezervace."))?,
                    );
                }
                "identity" if identity.is_none() => {
                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|_| bad("Soubor se nepodařilo načíst."))?;
                    identity = Some(bytes.to_vec());
                }
                _ => return Err(bad("Neznámé nebo opakované pole formuláře.")),
            }
        }
        Ok((
            input.ok_or_else(|| bad("Chybí údaje rezervace."))?,
            identity,
        ))
    })
    .await
    .map_err(|_| bad("Nahrávání trvalo příliš dlouho, zkuste to znovu."))??;
    let identity = process_image_files(image_slot, move || {
        identity
            .map(|bytes| {
                check_image(&bytes)?;
                sanitize_meter_photo(&bytes)
            })
            .transpose()
    })
    .await?;
    Ok((input, identity))
}

pub async fn create_admin_rental(
    State(s): State<Backend>,
    admin: auth::Admin,
    request: Request,
) -> Result<(StatusCode, Json<AdminCreated>)> {
    // Reject insufficient permission before reading a potentially large identity upload.
    let mut access = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut access).await?;
    require_rental_write_access(&mut access, &admin).await?;
    access.commit().await?;
    let (input, identity) = admin_rental_form(&s, request).await?;
    let now = OffsetDateTime::now_utc();
    input.periods(now)?;
    let (name, email, phone, details) = input.contact()?;
    if input.tenant_type == "organization" && (identity.is_none() || !input.consent) {
        return Err(bad(
            "U firmy nebo organizace nahrajte kopii dokladu odpovědné osoby a potvrďte její souhlas.",
        ));
    }
    if identity.is_some() && !input.consent {
        return Err(bad(
            "Potvrďte souhlas s pořízením a uložením kopie dokladu.",
        ));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let asset: Option<(i64, String)> =
        sqlx::query_as("SELECT id,name FROM rental_assets WHERE id=$1 FOR UPDATE")
            .bind(input.asset_id)
            .fetch_optional(&mut *tx)
            .await?;
    let (_, asset_name) = asset.ok_or_else(missing)?;
    let rates = asset_rates(&mut tx, input.asset_id).await?;
    let rate = rates.get(&input.event_type);
    let now = OffsetDateTime::now_utc();
    let periods = input.periods(now)?;
    let series_id = if input.recurring() {
        let id:i64=sqlx::query_scalar("INSERT INTO rental_series(asset_id,interval_days,interval_months,repeat_until,created_by,created_at) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id")
            .bind(input.asset_id).bind((input.recurrence_interval_days > 0).then_some(input.recurrence_interval_days)).bind((input.recurrence_interval_months > 0).then_some(input.recurrence_interval_months)).bind(date(input.recurrence_until.as_deref().unwrap_or(""))?).bind(admin.id).bind(timestamp(now)).fetch_one(&mut *tx).await?;
        Some(id)
    } else {
        None
    };
    let mut rentals = Vec::with_capacity(periods.len());
    let mut confirmations = Vec::with_capacity(periods.len());
    for period in &periods {
        let current = OffsetDateTime::now_utc();
        if period.has_started(current)? {
            return Err(conflict("Vybraný termín už začal. Zvolte nový čas."));
        }
        expire_pending(&mut tx, current, Some(input.asset_id), None).await?;
        ensure_available(&mut tx, input.asset_id, &period).await?;
        let current = OffsetDateTime::now_utc();
        if period.has_started(current)? {
            return Err(conflict("Vybraný termín už začal. Zvolte nový čas."));
        }
        let id:i64=sqlx::query_scalar("INSERT INTO rentals(asset_id,start_date,end_date,start_minute,end_minute,name,email,phone,address,status,rental_price,created_at,require_meter,source,wait_for_payment,series_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'approved',$10,$11,$12,'admin',$13,$14) RETURNING id")
            .bind(input.asset_id).bind(period.start).bind(period.end).bind(period.start_minute).bind(period.end_minute).bind(&name).bind(&email).bind(&phone).bind(&details.address).bind(input.rental_price).bind(timestamp(now)).bind(input.require_meter).bind(input.wait_for_payment).bind(series_id).fetch_one(&mut *tx).await?;
        tenant_snapshot(&mut tx, id, &input.tenant_type, &input.event_type, rate).await?;
        save_tenant_details(&mut tx, id, &details).await?;
        if let Some(identity) = &identity {
            sqlx::query("UPDATE rentals SET identity=$2,identity_type='image/jpeg',identity_consent_at=$3 WHERE id=$1")
                .bind(id).bind(identity).bind(timestamp(now)).execute(&mut *tx).await?;
        }
        let meter = if input.require_meter {
            let token = auth::token();
            sqlx::query("INSERT INTO rental_access_tokens(rental_id,token_hash) VALUES ($1,$2)")
                .bind(id)
                .bind(auth::hash(&token))
                .execute(&mut *tx)
                .await?;
            format!(
                "\nFotografie elektroměru před a po pronájmu nahrajte zde: {}\nOdkaz nikomu nepředávejte.",
                access_link(&s, &token)
            )
        } else {
            String::new()
        };
        confirmations.push(format!(
            "Rezervace č. {}: {}. Sjednaná cena: {} Kč.{}",
            id,
            period.label(),
            input.rental_price,
            meter
        ));
        if series_id.is_none() {
            queue(&mut tx,id,&email,"Rezervace pronájmu potvrzena",&format!("Dobrý den,\n\nsprávce obce pro Vás potvrdil pronájem {}.\n{}\n\nDěkujeme.\nObec Vyskeř",single_line(&asset_name),confirmations.last().unwrap()),"approval",now.unix_timestamp()).await?;
        }
        audit(
            &mut tx,
            Some(admin.id),
            "created_by_admin",
            "rental",
            id,
            now,
        )
        .await?;
        rentals.push(row(&mut tx, id).await?);
    }
    if let Some(id) = series_id {
        audit(&mut tx, Some(admin.id), "created", "rental_series", id, now).await?;
        queue(&mut tx,rentals[0].id,&email,"Pravidelné rezervace potvrzeny",&format!("Dobrý den,\n\nsprávce obce pro Vás potvrdil následující pravidelné pronájmy {}. Každý termín bude vyúčtován samostatně po skončení.\n\n{}\n\nDěkujeme.\nObec Vyskeř",single_line(&asset_name),confirmations.join("\n\n")),"series_approval",now.unix_timestamp()).await?;
    }
    // In a long series the first term can expire while later terms and messages are written.
    let current = OffsetDateTime::now_utc();
    for period in &periods {
        if period.has_started(current)? {
            return Err(conflict("Vybraný termín už začal. Zvolte nový čas."));
        }
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(AdminCreated { series_id, rentals }),
    ))
}

async fn cancel_one(
    conn: &mut PgConnection,
    s: &Backend,
    id: i64,
    admin_id: i64,
    _now: OffsetDateTime,
    notify: bool,
) -> Result<()> {
    sqlx::query("SELECT id FROM rentals WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let r = row(conn, id).await?;
    let now = OffsetDateTime::now_utc();
    if r.source != "admin"
        || r.status != "approved"
        || Period::from_rental(&r)?.start_instant()? <= now
    {
        return Err(conflict(
            "Zrušit lze pouze budoucí rezervaci zadanou správcem, která ještě nebyla vyúčtována.",
        ));
    }
    sqlx::query(
        "UPDATE rentals SET status='cancelled',identity=NULL,identity_type=NULL WHERE id=$1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM rental_access_tokens WHERE rental_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$2,body=NULL,invoice_attachment=NULL WHERE rental_id=$1 AND sent_at IS NULL AND cancelled_at IS NULL AND purpose<>'series_approval'").bind(id).bind(now.unix_timestamp()).execute(&mut *conn).await?;
    if notify {
        queue(conn,id,&r.email,"Rezervace pronájmu zrušena",&format!("Dobrý den,\n\nrezervace č. {} pro {} v termínu {} byla správcem zrušena. V případě dotazů kontaktujte obecní úřad.\n\nObec Vyskeř",id,single_line(&r.asset_name),Period::from_rental(&r)?.label()),"cancellation",now.unix_timestamp()).await?;
    }
    let _ = s;
    audit(conn, Some(admin_id), "cancelled", "rental", id, now).await?;
    Ok(())
}
pub async fn cancel_admin_rental(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<Rental>> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let asset_id: i64 = sqlx::query_scalar("SELECT asset_id FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    sqlx::query("SELECT id FROM rental_assets WHERE id=$1 FOR UPDATE")
        .bind(asset_id)
        .execute(&mut *tx)
        .await?;
    cancel_one(&mut tx, &s, id, admin.id, OffsetDateTime::now_utc(), true).await?;
    let result = row(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(result))
}
pub async fn cancel_series(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let asset_id: i64 = sqlx::query_scalar("SELECT asset_id FROM rental_series WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    sqlx::query("SELECT id FROM rental_assets WHERE id=$1 FOR UPDATE")
        .bind(asset_id)
        .execute(&mut *tx)
        .await?;
    let now = OffsetDateTime::now_utc();
    let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM rentals WHERE series_id=$1 AND source='admin' AND status='approved' AND (starts_local AT TIME ZONE 'Europe/Prague')>$2 ORDER BY id FOR UPDATE").bind(id).bind(now).fetch_all(&mut *tx).await?;
    let mut cancelled = Vec::with_capacity(ids.len());
    for rental_id in &ids {
        let rental = row(&mut tx, *rental_id).await?;
        cancelled.push(format!(
            "Rezervace č. {}: {}",
            rental_id,
            Period::from_rental(&rental)?.label()
        ));
        cancel_one(&mut tx, &s, *rental_id, admin.id, now, false).await?;
    }
    if let Some(first) = ids.first() {
        sqlx::query("UPDATE rental_mail_queue q SET cancelled_at=$2,body=NULL WHERE q.purpose='series_approval' AND q.sent_at IS NULL AND q.cancelled_at IS NULL AND EXISTS(SELECT 1 FROM rentals r WHERE r.id=q.rental_id AND r.series_id=$1)").bind(id).bind(now.unix_timestamp()).execute(&mut *tx).await?;
        let rental = row(&mut tx, *first).await?;
        queue(&mut tx,*first,&rental.email,"Pravidelné rezervace zrušeny",&format!("Dobrý den,\n\nsprávce zrušil následující budoucí termíny pronájmu {}:\n\n{}\n\nOstatní termíny a dřívější vyúčtování zůstávají beze změny. V případě dotazů kontaktujte obecní úřad.\n\nObec Vyskeř",single_line(&rental.asset_name),cancelled.join("\n")),"series_cancellation",now.unix_timestamp()).await?;
    }
    audit(
        &mut tx,
        Some(admin.id),
        "future_cancelled",
        "rental_series",
        id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({"cancelled_count":ids.len()})))
}
async fn row(conn: &mut PgConnection, id: i64) -> Result<Rental> {
    Ok(sqlx::query_as(rental_select!(" WHERE r.id=$1"))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(missing)?)
}
pub async fn admin_detail(
    State(s): State<Backend>,
    _: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<Rental>> {
    let mut c = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut c).await?;
    Ok(Json(row(&mut c, id).await?))
}
fn access_link(s: &Backend, token: &str) -> String {
    format!("{}/pronajmy/meridlo/{}", s.config.public_url, token)
}
pub async fn approve(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<Rental>> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    sqlx::query("SELECT a.id FROM rental_assets a JOIN rentals r ON r.asset_id=a.id WHERE r.id=$1 FOR UPDATE OF a")
        .bind(id).execute(&mut *tx).await?;
    sqlx::query("SELECT id FROM rentals WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let r = row(&mut tx, id).await?;
    let now = OffsetDateTime::now_utc();
    let period = Period::from_rental(&r)?;
    if r.status == "pending" && period.start_instant()? <= now {
        expire_pending(&mut tx, now, None, Some(id)).await?;
        tx.commit().await?;
        return Err(conflict(
            "Žádost nebyla potvrzena do začátku pronájmu a byla automaticky zrušena.",
        ));
    }
    let token = auth::token();
    let changed =
        sqlx::query("UPDATE rentals SET status='approved' WHERE id=$1 AND status='pending'")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if changed == 0 {
        return Err(conflict("Žádost již nelze schválit."));
    }
    cancel_application_messages(&mut tx, id, now.unix_timestamp()).await?;
    sqlx::query("INSERT INTO rental_access_tokens(rental_id,token_hash) VALUES ($1,$2)")
        .bind(id)
        .bind(auth::hash(&token))
        .execute(&mut *tx)
        .await?;
    let requires: bool = sqlx::query_scalar("SELECT require_meter FROM rentals WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let meter = if requires {
        format!(
            "\nFotografie elektroměru před a po pronájmu nahrajte zde: {}\nOdkaz nikomu nepředávejte.",
            access_link(&s, &token)
        )
    } else {
        String::new()
    };
    queue(&mut tx,id,&r.email,"Žádost o pronájem schválena",&format!("Vaše žádost o pronájem {} v termínu {} byla schválena.\nKlíče si vyzvedněte na obecním úřadě v pracovní den před začátkem nájmu. Vraťte je poslední den nájmu nebo následující pracovní den na začátku otevírací doby.{}",single_line(&r.asset_name),period.label(),meter),"approval",now.unix_timestamp()).await?;
    audit(&mut tx, Some(admin.id), "approved", "rental", id, now).await?;
    let r = row(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(r))
}
pub async fn reject(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<Rental>> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    sqlx::query("SELECT id FROM rentals WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let r = row(&mut tx, id).await?;
    let now = OffsetDateTime::now_utc();
    if r.status == "pending" && Period::from_rental(&r)?.start_instant()? <= now {
        expire_pending(&mut tx, now, None, Some(id)).await?;
        tx.commit().await?;
        return Err(conflict(
            "Žádost nebyla potvrzena do začátku pronájmu a byla automaticky zrušena.",
        ));
    }
    let changed=sqlx::query("UPDATE rentals SET status='rejected',identity=NULL,identity_type=NULL WHERE id=$1 AND status='pending'").bind(id).execute(&mut *tx).await?.rows_affected();
    if changed == 0 {
        return Err(conflict("Žádost již nelze zamítnout."));
    }
    cancel_application_messages(&mut tx, id, now.unix_timestamp()).await?;
    queue(&mut tx,id,&r.email,"Žádost o pronájem",&format!("Vaše žádost o pronájem {} v termínu {} nebyla schválena. Omlouváme se. V případě dotazů kontaktujte obecní úřad.",single_line(&r.asset_name),Period::from_rental(&r)?.label()),"rejected",now.unix_timestamp()).await?;
    audit(&mut tx, Some(admin.id), "rejected", "rental", id, now).await?;
    let r = row(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(r))
}
#[derive(Serialize)]
pub struct Access {
    asset_name: String,
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    before_uploaded: bool,
    after_uploaded: bool,
    before_photo_taken_at: Option<String>,
    after_photo_taken_at: Option<String>,
}
pub async fn access(State(s): State<Backend>, Path(token): Path<String>) -> Result<Json<Access>> {
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(missing());
    }
    Ok(Json(access_data(&mut tx, &auth::hash(token)).await?))
}
async fn access_data(conn: &mut PgConnection, token_hash: &str) -> Result<Access> {
    Ok(sqlx::query_as::<_,AccessRow>("SELECT a.name AS asset_name,r.start_date::text,r.end_date::text,CASE WHEN r.start_minute IS NOT NULL THEN lpad((r.start_minute/60)::text,2,'0')||':00' END AS start_time,CASE WHEN r.end_minute IS NOT NULL THEN lpad((r.end_minute/60)::text,2,'0')||':00' END AS end_time,r.before_photo IS NOT NULL AS before_uploaded,r.after_photo IS NOT NULL AS after_uploaded,r.before_photo_taken_at,r.after_photo_taken_at FROM rentals r JOIN rental_assets a ON a.id=r.asset_id WHERE EXISTS(SELECT 1 FROM rental_access_tokens t WHERE t.rental_id=r.id AND t.token_hash=$1) AND r.status='approved' AND r.require_meter").bind(token_hash).fetch_optional(conn).await?.ok_or_else(missing)?.into())
}
#[derive(sqlx::FromRow)]
struct AccessRow {
    asset_name: String,
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    before_uploaded: bool,
    after_uploaded: bool,
    before_photo_taken_at: Option<String>,
    after_photo_taken_at: Option<String>,
}
impl From<AccessRow> for Access {
    fn from(r: AccessRow) -> Self {
        Self {
            asset_name: r.asset_name,
            start_date: r.start_date,
            end_date: r.end_date,
            start_time: r.start_time,
            end_time: r.end_time,
            before_uploaded: r.before_uploaded,
            after_uploaded: r.after_uploaded,
            before_photo_taken_at: r.before_photo_taken_at,
            after_photo_taken_at: r.after_photo_taken_at,
        }
    }
}

struct MeterPhoto {
    bytes: Vec<u8>,
    taken_at: Option<String>,
}
fn prepare_meter_photo(bytes: Vec<u8>) -> Result<MeterPhoto> {
    check_image(&bytes)?;
    Ok(MeterPhoto {
        taken_at: photo_taken_at(&bytes),
        bytes: sanitize_meter_photo(&bytes)?,
    })
}
async fn meter_field(
    mut field: axum::extract::multipart::Field<'_>,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| bad("Soubor se nepodařilo načíst."))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(bad(if limit == FILE_LIMIT {
                "Soubor může mít nejvýše 10 MB."
            } else {
                "Neplatná pole formuláře."
            }));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub async fn upload(
    State(s): State<Backend>,
    headers: HeaderMap,
    ip: auth::ClientIp,
    Path(token): Path<String>,
    mut form: Multipart,
) -> Result<Json<Access>> {
    auth::check_origin(&headers, &s)?;
    auth::throttle(
        &s,
        &format!("rental-photo:{}", ip.0),
        30,
        3600,
        OffsetDateTime::now_utc(),
    )
    .await?;
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(missing());
    }
    let token_hash = auth::hash(&token);
    let mut check = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut check).await?;
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rental_access_tokens t JOIN rentals r ON r.id=t.rental_id WHERE t.token_hash=$1 AND r.status='approved' AND r.require_meter)")
        .bind(&token_hash).fetch_one(&mut *check).await?;
    check.commit().await?;
    if !valid {
        return Err(missing());
    }
    let image_slot =
        tokio::time::timeout(std::time::Duration::from_secs(60), IMAGE_SLOTS.acquire())
            .await
            .map_err(|_| bad("Zpracování souborů je zaneprázdněné, zkuste to znovu."))?
            .map_err(|_| bad("Soubor nelze zpracovat."))?;
    let (before, after) = tokio::time::timeout(std::time::Duration::from_secs(60), async move {
        let mut kind = None;
        let mut photo = None;
        let mut before = None;
        let mut after = None;
        while let Some(field) = form
            .next_field()
            .await
            .map_err(|_| bad("Neplatný formulář."))?
        {
            match field.name().unwrap_or("") {
                "kind" if kind.is_none() => {
                    kind = Some(
                        String::from_utf8(meter_field(field, 16).await?)
                            .map_err(|_| bad("Neplatná pole formuláře."))?,
                    )
                }
                "photo" if photo.is_none() => photo = Some(meter_field(field, FILE_LIMIT).await?),
                "before" if before.is_none() => {
                    before = Some(meter_field(field, FILE_LIMIT).await?)
                }
                "after" if after.is_none() => after = Some(meter_field(field, FILE_LIMIT).await?),
                _ => return Err(bad("Neplatná pole formuláře.")),
            }
        }
        // Keep the original single-photo API, but never combine its fields with the new form.
        if kind.is_some() || photo.is_some() {
            if before.is_some() || after.is_some() {
                return Err(bad("Neplatná pole formuláře."));
            }
            let photo = photo.ok_or_else(|| bad("Nahrajte fotografii."))?;
            match kind.as_deref() {
                Some("before") => before = Some(photo),
                Some("after") => after = Some(photo),
                _ => return Err(bad("Vyberte fotografii před nebo po pronájmu.")),
            }
        }
        if before.is_none() && after.is_none() {
            return Err(bad("Nahrajte alespoň jednu fotografii."));
        }
        Ok::<_, super::Error>((before, after))
    })
    .await
    .map_err(|_| bad("Nahrávání trvalo příliš dlouho, zkuste to znovu."))??;
    // Validate and sanitize every selected photo before changing any stored photo.
    let (before, after) = process_image_files(image_slot, move || {
        Ok((
            before.map(prepare_meter_photo).transpose()?,
            after.map(prepare_meter_photo).transpose()?,
        ))
    })
    .await?;
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let result: Option<(i64, bool)> = sqlx::query_as("UPDATE rentals SET before_photo=COALESCE($1,before_photo),before_type=CASE WHEN $1::bytea IS NOT NULL THEN 'image/jpeg' ELSE before_type END,before_photo_taken_at=CASE WHEN $1::bytea IS NOT NULL THEN $2 ELSE before_photo_taken_at END,after_photo=COALESCE($3,after_photo),after_type=CASE WHEN $3::bytea IS NOT NULL THEN 'image/jpeg' ELSE after_type END,after_photo_taken_at=CASE WHEN $3::bytea IS NOT NULL THEN $4 ELSE after_photo_taken_at END WHERE EXISTS(SELECT 1 FROM rental_access_tokens t WHERE t.rental_id=rentals.id AND t.token_hash=$5) AND status='approved' AND require_meter RETURNING id,before_photo IS NOT NULL AND after_photo IS NOT NULL")
        .bind(before.as_ref().map(|p| p.bytes.as_slice()))
        .bind(before.as_ref().and_then(|p| p.taken_at.as_deref()))
        .bind(after.as_ref().map(|p| p.bytes.as_slice()))
        .bind(after.as_ref().and_then(|p| p.taken_at.as_deref()))
        .bind(&token_hash)
        .fetch_optional(&mut *tx)
        .await?;
    let (rental_id, complete) = result.ok_or_else(missing)?;
    if complete {
        sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$1,body=NULL WHERE rental_id=$2 AND sent_at IS NULL AND cancelled_at IS NULL AND purpose IN ('reminder','reminder_admin')")
            .bind(OffsetDateTime::now_utc().unix_timestamp()).bind(rental_id).execute(&mut *tx).await?;
    }
    let access = access_data(&mut tx, &token_hash).await?;
    tx.commit().await?;
    Ok(Json(access))
}

pub async fn access_photo(
    State(s): State<Backend>,
    Path((token, kind)): Path<(String, String)>,
) -> Result<Response> {
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(missing());
    }
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let sql = match kind.as_str() {
        "before" => {
            "SELECT before_photo,before_type FROM rentals r WHERE EXISTS(SELECT 1 FROM rental_access_tokens t WHERE t.rental_id=r.id AND t.token_hash=$1) AND r.status='approved' AND r.require_meter"
        }
        "after" => {
            "SELECT after_photo,after_type FROM rentals r WHERE EXISTS(SELECT 1 FROM rental_access_tokens t WHERE t.rental_id=r.id AND t.token_hash=$1) AND r.status='approved' AND r.require_meter"
        }
        _ => return Err(missing()),
    };
    let data: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(sql)
        .bind(auth::hash(token))
        .fetch_optional(&mut *tx)
        .await?;
    let (Some(bytes), Some(mime)) = data.ok_or_else(missing)? else {
        return Err(missing());
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (header::CONTENT_DISPOSITION, "inline".to_owned()),
        ],
        Body::from(bytes),
    )
        .into_response())
}
async fn attachment(s: &Backend, id: i64, kind: &str) -> Result<Response> {
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let sql = match kind {
        "before" => "SELECT before_photo,before_type FROM rentals WHERE id=$1",
        "after" => "SELECT after_photo,after_type FROM rentals WHERE id=$1",
        _ => return Err(missing()),
    };
    let data: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(sql)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let (Some(bytes), Some(mime)) = data.ok_or_else(missing)? else {
        return Err(missing());
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (header::CONTENT_DISPOSITION, "inline".to_owned()),
        ],
        Body::from(bytes),
    )
        .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityAccess {
    password: String,
}

pub async fn identity(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<IdentityAccess>,
) -> Result<Response> {
    if !admin.can_view_identity {
        return Err(super::Error(
            StatusCode::FORBIDDEN,
            "Nemáte oprávnění zobrazovat kopie dokladů.",
        ));
    }
    let now = OffsetDateTime::now_utc();
    auth::throttle(
        &s,
        &format!("rental-identity-reauth:{}", admin.id),
        30,
        900,
        now,
    )
    .await?;
    let stored: String =
        sqlx::query_scalar("SELECT password_hash FROM administrators WHERE id=$1 AND active=TRUE")
            .bind(admin.id)
            .fetch_optional(&s.pool)
            .await?
            .ok_or_else(auth::denied)?;
    auth::verify_password(input.password, stored.clone()).await?;

    // Recheck the session and password under the same write lock used for account changes.
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    super::accounts::current(&mut tx, &admin, &stored).await?;
    super::accounts::require_permission(&mut tx, admin.id, "identity").await?;
    let data: Option<(Option<Vec<u8>>, Option<String>)> =
        sqlx::query_as("SELECT identity,identity_type FROM rentals WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (Some(bytes), Some(mime)) = data.ok_or_else(missing)? else {
        return Err(missing());
    };
    audit(
        &mut tx,
        Some(admin.id),
        "viewed_identity",
        "rental",
        id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (header::CONTENT_DISPOSITION, "inline".to_owned()),
        ],
        Body::from(bytes),
    )
        .into_response())
}
pub async fn photo(
    State(s): State<Backend>,
    _: auth::Admin,
    Path((id, kind)): Path<(i64, String)>,
) -> Result<Response> {
    if kind != "before" && kind != "after" {
        return Err(missing());
    }
    attachment(&s, id, &kind).await
}
#[derive(Deserialize)]
pub struct Invoice {
    energy_kwh: String,
    energy_cost: i64,
    extra_cost: i64,
    #[serde(default)]
    account_number: String,
    #[serde(default)]
    bank_code: String,
    #[serde(default)]
    account_iban: String,
    due_days: i32,
}
pub async fn invoice(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<Invoice>,
) -> Result<Json<Rental>> {
    invoice_inner(s, admin, id, input, None).await
}

// This is a type check, not a claim that the PDF is safe or a valid invoice.
// Checking both the version header and the closing marker catches truncated
// files and common mistakes without running an untrusted PDF parser.
fn looks_like_pdf(bytes: &[u8]) -> bool {
    let header_ok = bytes.len() >= 9
        && bytes.starts_with(b"%PDF-")
        && matches!(bytes[5], b'1' | b'2')
        && bytes[6] == b'.'
        && bytes[7].is_ascii_digit()
        && matches!(bytes[8], b'\r' | b'\n');
    let last = bytes.iter().rposition(|b| !b.is_ascii_whitespace());
    header_ok && last.is_some_and(|end| bytes[..=end].ends_with(b"%%EOF"))
}

pub async fn invoice_with_attachment(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    mut form: Multipart,
) -> Result<Json<Rental>> {
    // Admin extraction validates the current session and CSRF before the body
    // is read. The permission is checked again under the write transaction.
    if !admin.can_view_identity {
        return Err(super::Error(
            StatusCode::FORBIDDEN,
            "K této akci nemáte oprávnění.",
        ));
    }
    let _slot = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        INVOICE_UPLOAD_SLOTS.acquire(),
    )
    .await
    .map_err(|_| bad("Zpracování souborů je zaneprázdněné, zkuste to znovu."))?
    .map_err(|_| bad("Soubor nelze zpracovat."))?;
    let (input, pdf) = tokio::time::timeout(std::time::Duration::from_secs(60), async move {
        let mut input = None;
        let mut pdf = None;
        while let Some(mut field) = form
            .next_field()
            .await
            .map_err(|_| bad("Neplatný formulář."))?
        {
            let name = field.name().unwrap_or("").to_owned();
            let limit = match name.as_str() {
                "invoice" if input.is_none() => 4096,
                "attachment" if pdf.is_none() => FILE_LIMIT,
                "invoice" | "attachment" => return Err(bad("Pole je zadáno vícekrát.")),
                _ => return Err(bad("Neznámé pole formuláře.")),
            };
            let mut bytes = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| bad("Soubor se nepodařilo načíst."))?
            {
                if chunk.len() > limit - bytes.len() {
                    return Err(bad("Soubor nebo pole je příliš velké."));
                }
                bytes.extend_from_slice(&chunk);
            }
            if name == "invoice" {
                input = Some(
                    serde_json::from_slice::<Invoice>(&bytes)
                        .map_err(|_| bad("Neplatné údaje vyúčtování."))?,
                );
            } else {
                if !looks_like_pdf(&bytes) {
                    return Err(bad("Nahrajte platný soubor PDF."));
                }
                pdf = Some(bytes);
            }
        }
        Ok::<_, super::Error>((
            input.ok_or_else(|| bad("Chybí údaje vyúčtování."))?,
            pdf.ok_or_else(|| bad("Chybí příloha PDF."))?,
        ))
    })
    .await
    .map_err(|_| bad("Nahrávání trvalo příliš dlouho, zkuste to znovu."))??;
    invoice_inner(s, admin, id, input, Some(pdf)).await
}

async fn invoice_inner(
    s: Backend,
    admin: auth::Admin,
    id: i64,
    input: Invoice,
    attachment: Option<Vec<u8>>,
) -> Result<Json<Rental>> {
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let result = issue_invoice(&mut tx, id, input, attachment, Some(admin.id), now).await?;
    tx.commit().await?;
    Ok(Json(result))
}

async fn issue_invoice(
    conn: &mut PgConnection,
    id: i64,
    input: Invoice,
    attachment: Option<Vec<u8>>,
    actor: Option<i64>,
    now: OffsetDateTime,
) -> Result<Rental> {
    let kwh = input.energy_kwh.trim().replace(',', ".");
    if kwh.len() > 30
        || kwh.is_empty()
        || !kwh.chars().all(|c| c.is_ascii_digit() || c == '.')
        || kwh.matches('.').count() > 1
        || kwh
            .parse::<f64>()
            .ok()
            .is_none_or(|n| !n.is_finite() || !(0.0..=100_000_000.0).contains(&n))
    {
        return Err(bad("Zadejte platnou spotřebu v kWh."));
    }
    if !(0..=100_000_000).contains(&input.energy_cost)
        || !(0..=100_000_000).contains(&input.extra_cost)
        || !(1..=90).contains(&input.due_days)
    {
        return Err(bad("Neplatná částka nebo splatnost."));
    }
    // Accept older clients that only sent an IBAN, while new forms send the
    // domestic account and optional international representation explicitly.
    let payment = if input.account_number.trim().is_empty()
        && input.bank_code.trim().is_empty()
        && !input.account_iban.trim().is_empty()
    {
        let iban = super::rental_payment::normalized_iban(&input.account_iban)
            .map_err(|_| bad("Zadejte platný IBAN."))?;
        if iban.starts_with("CZ") {
            let domestic = format!("{}-{}", &iban[8..14], &iban[14..24]);
            super::rental_payment_settings::validate_payment_details(&domestic, &iban[4..8], &iban)?
        } else {
            return Err(bad("Zadejte české číslo účtu a kód banky."));
        }
    } else {
        super::rental_payment_settings::validate_payment_details(
            &input.account_number,
            &input.bank_code,
            &input.account_iban,
        )?
    };
    let r = row(&mut *conn, id).await?;
    if r.status != "approved" || Period::from_rental(&r)?.end_instant()? > now {
        return Err(conflict(
            "Vyúčtovat lze pouze schválený nájem po jeho skončení.",
        ));
    }
    let requires: bool =
        sqlx::query_scalar("SELECT require_meter FROM rentals WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    if !requires && (input.energy_cost != 0 || kwh.parse::<f64>().unwrap_or(-1.0) != 0.0) {
        return Err(bad(
            "U tohoto pronájmu se energie nevyúčtovávají. Spotřeba i cena energie musí být nulové.",
        ));
    }
    if requires && (!r.before_uploaded || !r.after_uploaded) {
        return Err(conflict(
            "Nejdříve musí být nahrány obě fotografie elektroměru.",
        ));
    }
    let total = r
        .rental_price
        .checked_add(input.energy_cost)
        .and_then(|n| n.checked_add(input.extra_cost))
        .ok_or_else(|| bad("Neplatná celková částka."))?;
    if total > 0 {
        super::rental_payment::qr_png(&payment.effective_iban, total, id)
            .map_err(|_| bad("Zadejte platné číslo účtu a kód banky."))?;
    }
    let issued = today(now);
    let due = issued + Duration::days(i64::from(input.due_days));
    let changed=sqlx::query("UPDATE rentals SET status='invoiced',energy_kwh=$1,energy_cost=$2,extra_cost=$3,total_price=$4,account_iban=$5,due_days=$6,invoice_sent_at=NULL,account_number=$8,bank_code=$9,payment_iban=$10,invoice_issued_on=$11,invoice_due_on=$12,billing_error=NULL WHERE id=$7 AND status='approved'")
        .bind(&kwh).bind(input.energy_cost).bind(input.extra_cost).bind(total).bind(&payment.effective_iban).bind(input.due_days).bind(id)
        .bind(&payment.account_number).bind(&payment.bank_code).bind(&payment.iban).bind(issued).bind(due).execute(&mut *conn).await?.rows_affected();
    if changed == 0 {
        return Err(conflict("Nájem již nelze vyúčtovat."));
    }
    sqlx::query("DELETE FROM rental_access_tokens WHERE rental_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$1,body=NULL WHERE rental_id=$2 AND sent_at IS NULL AND purpose IN ('reminder','reminder_admin')").bind(now.unix_timestamp()).bind(id).execute(&mut *conn).await?;
    let body =
        super::rental_invoice_mail::render_text(&super::rental_invoice_mail::InvoiceDetails {
            id,
            asset_name: &r.asset_name,
            renter_name: &r.name,
            renter_address: &r.address,
            organization_ico: r.organization_ico.as_deref(),
            responsible_name: r.responsible_name.as_deref(),
            start_date: &r.start_date,
            end_date: &r.end_date,
            start_time: r.start_time.as_deref(),
            end_time: r.end_time.as_deref(),
            rental_price: r.rental_price,
            energy_kwh: &kwh,
            energy_cost: input.energy_cost,
            extra_cost: input.extra_cost,
            total,
            account_number: &payment.account_number,
            bank_code: &payment.bank_code,
            iban: &payment.iban,
            issued_on: issued,
            due_date: due,
        });
    let queue_id = queue(
        &mut *conn,
        id,
        &r.email,
        "Vyúčtování pronájmu",
        &body,
        "invoice",
        now.unix_timestamp(),
    )
    .await?;
    if let Some(pdf) = attachment {
        sqlx::query("UPDATE rental_mail_queue SET invoice_attachment=$1 WHERE id=$2")
            .bind(pdf)
            .bind(queue_id)
            .execute(&mut *conn)
            .await?;
    }
    audit(&mut *conn, actor, "invoiced", "rental", id, now).await?;
    let r = row(&mut *conn, id).await?;
    Ok(r)
}
pub async fn paid(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Json<Rental>> {
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    require_rental_write_access(&mut tx, &admin).await?;
    let updated=sqlx::query("UPDATE rentals SET status='paid',identity=NULL,identity_type=NULL WHERE id=$1 AND status='invoiced' AND wait_for_payment").bind(id).execute(&mut *tx).await?.rows_affected();
    if updated == 0 {
        return Err(conflict("Uzavřít lze pouze vyúčtovaný nájem."));
    }
    sqlx::query("DELETE FROM rental_access_tokens WHERE rental_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$1,body=NULL WHERE rental_id=$2 AND sent_at IS NULL AND purpose IN ('reminder','reminder_admin')").bind(now.unix_timestamp()).bind(id).execute(&mut *tx).await?;
    audit(&mut tx, Some(admin.id), "paid", "rental", id, now).await?;
    let r = row(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(r))
}
#[derive(sqlx::FromRow)]
struct Reminder {
    id: i64,
    email: String,
    notification_email: String,
    asset_name: String,
}
pub async fn maintenance(s: &Backend, now: OffsetDateTime) -> Result<()> {
    let day = today(now);
    let mut tx = crate::db::begin_write(&s.pool).await?;
    if !super::rental_settings::enabled_locked(&mut tx).await? {
        return Ok(());
    }
    expire_pending(&mut tx, now, None, None).await?;
    let items:Vec<Reminder>=sqlx::query_as("SELECT r.id,r.email,a.notification_email,a.name AS asset_name FROM rentals r JOIN rental_assets a ON a.id=r.asset_id WHERE r.status='approved' AND r.require_meter AND (r.ends_local AT TIME ZONE 'Europe/Prague')<=$2 AND (r.before_photo IS NULL OR r.after_photo IS NULL) AND (r.last_reminder_date IS NULL OR r.last_reminder_date<$1) FOR UPDATE OF r SKIP LOCKED LIMIT 100").bind(day).bind(now).fetch_all(&mut *tx).await?;
    for r in items {
        // Keep only today's reminder after a prolonged SMTP outage.
        sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$2,body=NULL WHERE rental_id=$1 AND purpose IN ('reminder','reminder_admin') AND sent_at IS NULL AND cancelled_at IS NULL")
            .bind(r.id).bind(now.unix_timestamp()).execute(&mut *tx).await?;
        let token = auth::token();
        sqlx::query("UPDATE rentals SET last_reminder_date=$1 WHERE id=$2")
            .bind(day)
            .bind(r.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO rental_access_tokens(rental_id,token_hash) VALUES ($1,$2)")
            .bind(r.id)
            .bind(auth::hash(&token))
            .execute(&mut *tx)
            .await?;
        queue(&mut tx,r.id,&r.email,"Výzva k nahrání fotografií elektroměru",&format!("Pronájem {} skončil. Nahrajte prosím fotografie stavu elektroměru před a po pronájmu: {}\nOdkaz nepředávejte dalším osobám.",single_line(&r.asset_name),access_link(s,&token)),"reminder",now.unix_timestamp()).await?;
        queue(&mut tx,r.id,&r.notification_email,"Chybí fotografie elektroměru",&format!("U pronájmu č. {} ({}) dosud chybí fotografie elektroměru. Více informací najdete v administraci.",r.id,single_line(&r.asset_name)),"reminder_admin",now.unix_timestamp()).await?;
    }
    let automatic:Vec<(i64,Option<String>,String)>=sqlx::query_as("SELECT r.id,r.billing_error,a.notification_email FROM rentals r JOIN rental_assets a ON a.id=r.asset_id WHERE r.status='approved' AND r.source='admin' AND r.series_id IS NOT NULL AND NOT r.require_meter AND (r.ends_local AT TIME ZONE 'Europe/Prague')<=$1 AND r.auto_invoice_retry_after<=$2 ORDER BY r.ends_local,r.id FOR UPDATE OF r SKIP LOCKED LIMIT 100")
        .bind(now).bind(now.unix_timestamp()).fetch_all(&mut *tx).await?;
    if !automatic.is_empty() {
        let (account_number,bank_code,account_iban,due_days):(String,String,String,i32)=sqlx::query_as("SELECT account_number,bank_code,iban,due_days FROM rental_payment_settings WHERE id=1").fetch_one(&mut *tx).await?;
        for (id, previous_error, notification_email) in automatic {
            let input = Invoice {
                energy_kwh: "0".into(),
                energy_cost: 0,
                extra_cost: 0,
                account_number: account_number.clone(),
                bank_code: bank_code.clone(),
                account_iban: account_iban.clone(),
                due_days,
            };
            match issue_invoice(&mut tx, id, input, None, None, now).await {
                Ok(_) => (),
                Err(error)
                    if error.0 == StatusCode::BAD_REQUEST || error.0 == StatusCode::CONFLICT =>
                {
                    sqlx::query("UPDATE rentals SET billing_error=$2,auto_invoice_retry_after=$3 WHERE id=$1")
                        .bind(id).bind(error.1).bind(now.unix_timestamp()+3600).execute(&mut *tx).await?;
                    if previous_error.as_deref() != Some(error.1) {
                        audit(
                            &mut tx,
                            None,
                            "automatic_invoice_needs_attention",
                            "rental",
                            id,
                            now,
                        )
                        .await?;
                        queue(&mut tx,id,&notification_email,"Vyúčtování vyžaduje kontrolu",&format!("Automatické vyúčtování pronájmu č. {id} nebylo možné připravit: {} Podrobnosti najdete v administraci.",error.1),"billing_attention",now.unix_timestamp()).await?;
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
    tx.commit().await?;
    Ok(())
}
#[derive(sqlx::FromRow)]
struct Pending {
    id: i64,
    rental_id: i64,
    recipient: String,
    subject: String,
    body: Option<String>,
    purpose: String,
    attempts: i64,
    created_at: i64,
    invoice_attachment: Option<Vec<u8>>,
}

async fn current_series_confirmation(
    conn: &mut PgConnection,
    s: &Backend,
    rental_id: i64,
    now: OffsetDateTime,
) -> Result<Option<String>> {
    let anchor = row(&mut *conn, rental_id).await?;
    let Some(series_id) = anchor.series_id else {
        return Ok(None);
    };
    let members:Vec<Rental>=sqlx::query_as(rental_select!(" WHERE r.series_id=$1 AND r.status='approved' AND (r.starts_local AT TIME ZONE 'Europe/Prague')>$2 ORDER BY r.start_date,r.id"))
        .bind(series_id).bind(now).fetch_all(&mut *conn).await?;
    if members.is_empty() {
        return Ok(None);
    }
    let mut lines = Vec::with_capacity(members.len());
    for member in members {
        let meter = if member.require_meter {
            let token = auth::token();
            sqlx::query("INSERT INTO rental_access_tokens(rental_id,token_hash) VALUES ($1,$2)")
                .bind(member.id)
                .bind(auth::hash(&token))
                .execute(&mut *conn)
                .await?;
            format!(
                "\nFotografie elektroměru před a po pronájmu nahrajte zde: {}\nOdkaz nikomu nepředávejte.",
                access_link(s, &token)
            )
        } else {
            String::new()
        };
        lines.push(format!(
            "Rezervace č. {}: {}. Sjednaná cena: {} Kč.{}",
            member.id,
            Period::from_rental(&member)?.label(),
            member.rental_price,
            meter
        ));
    }
    Ok(Some(format!(
        "Dobrý den,\n\nsprávce obce pro Vás potvrdil následující pravidelné pronájmy {}. Každý termín bude vyúčtován samostatně po skončení.\n\n{}\n\nDěkujeme.\nObec Vyskeř",
        single_line(&anchor.asset_name),
        lines.join("\n\n")
    )))
}
pub async fn deliver_one(
    s: &Backend,
    smtp: &AsyncSmtpTransport<Tokio1Executor>,
    now: OffsetDateTime,
) -> anyhow::Result<bool> {
    let mut tx = s.pool.begin().await?;
    if !super::rental_settings::enabled_locked(&mut tx)
        .await
        .map_err(|_| anyhow::anyhow!("Unable to read rental availability"))?
    {
        return Ok(false);
    }
    // Peek without locking the queue, then keep settings -> asset -> rental ->
    // queue lock order. This also keeps a single database connection per worker.
    let candidate: Option<(i64,i64,String)> = sqlx::query_as("SELECT id,rental_id,purpose FROM rental_mail_queue WHERE sent_at IS NULL AND cancelled_at IS NULL AND next_attempt_at<=$1 AND locked_until<=$1 ORDER BY id LIMIT 1")
        .bind(now.unix_timestamp()).fetch_optional(&mut *tx).await?;
    let Some((message_id, rental_id, purpose)) = candidate else {
        tx.commit().await?;
        return Ok(false);
    };
    if purpose == "series_approval" {
        sqlx::query("SELECT a.id FROM rental_assets a JOIN rentals r ON r.asset_id=a.id WHERE r.id=$1 FOR UPDATE OF a").bind(rental_id).execute(&mut *tx).await?;
    }
    sqlx::query("SELECT id FROM rentals WHERE id=$1 FOR UPDATE")
        .bind(rental_id)
        .execute(&mut *tx)
        .await?;
    // A queued message can wait behind an administrator or another SMTP delivery.
    // Recheck the real clock after that wait while retaining deterministic future-clock tests.
    let now = now.max(OffsetDateTime::now_utc());
    if purpose == "application"
        && expire_pending(&mut tx, now, None, Some(rental_id))
            .await
            .map_err(|_| anyhow::anyhow!("Unable to expire pending rental"))?
            > 0
    {
        tx.commit().await?;
        return Ok(true);
    }
    if matches!(purpose.as_str(), "application" | "approval") {
        // A retry can outlive the decision or settlement, including mail queued by
        // older versions. Recheck under the rental lock before any SMTP attempt.
        let current: String = sqlx::query_scalar("SELECT status FROM rentals WHERE id=$1")
            .bind(rental_id)
            .fetch_one(&mut *tx)
            .await?;
        let expected = if purpose == "application" {
            "pending"
        } else {
            "approved"
        };
        if current != expected {
            sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$2,body=NULL,lock_token=NULL,locked_until=0 WHERE id=$1 AND sent_at IS NULL AND cancelled_at IS NULL")
                .bind(message_id).bind(now.unix_timestamp()).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
    }
    let lease = auth::token();
    let message:Option<Pending>=sqlx::query_as("UPDATE rental_mail_queue SET lock_token=$1,locked_until=$2,attempts=attempts+1 WHERE id=$3 AND rental_id=$5 AND purpose=$6 AND sent_at IS NULL AND cancelled_at IS NULL AND next_attempt_at<=$4 AND locked_until<=$4 RETURNING id,rental_id,recipient,subject,body,purpose,attempts,created_at,invoice_attachment")
        .bind(&lease).bind(now.unix_timestamp()+300).bind(message_id).bind(now.unix_timestamp()).bind(rental_id).bind(purpose).fetch_optional(&mut *tx).await?;
    let Some(mut message) = message else {
        tx.commit().await?;
        return Ok(true);
    };
    let still_queued:bool=sqlx::query_scalar("SELECT COALESCE(cancelled_at IS NULL AND sent_at IS NULL AND lock_token=$2,FALSE) FROM rental_mail_queue WHERE id=$1 FOR UPDATE").bind(message.id).bind(&lease).fetch_one(&mut *tx).await?;
    if !still_queued {
        tx.commit().await?;
        return Ok(true);
    }
    if message.purpose == "series_approval" {
        // A failed summary must never confirm a member cancelled while SMTP was down.
        let body = current_series_confirmation(&mut tx, s, message.rental_id, now)
            .await
            .map_err(|_| anyhow::anyhow!("Unable to refresh rental series confirmation"))?;
        let Some(body) = body else {
            sqlx::query("UPDATE rental_mail_queue SET cancelled_at=$2,body=NULL,lock_token=NULL,locked_until=0 WHERE id=$1 AND lock_token=$3").bind(message.id).bind(now.unix_timestamp()).bind(&lease).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        };
        sqlx::query("UPDATE rental_mail_queue SET body=$2 WHERE id=$1 AND lock_token=$3")
            .bind(message.id)
            .bind(&body)
            .bind(&lease)
            .execute(&mut *tx)
            .await?;
        message.body = Some(body);
    }
    if matches!(message.purpose.as_str(), "reminder" | "reminder_admin") {
        // Serialize delivery with uploads, invoicing and cancellation. Once the
        // upload commits, a queued reminder can no longer begin SMTP delivery.
        let needed:bool=sqlx::query_scalar("SELECT status='approved' AND require_meter AND (before_photo IS NULL OR after_photo IS NULL) FROM rentals WHERE id=$1 FOR UPDATE").bind(message.rental_id).fetch_one(&mut *tx).await?;
        let message_day = today(OffsetDateTime::from_unix_timestamp(message.created_at)?);
        if !needed || message_day < today(now) {
            sqlx::query("UPDATE rental_mail_queue SET cancelled_at=COALESCE(cancelled_at,$1),body=NULL,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3").bind(now.unix_timestamp()).bind(message.id).bind(&lease).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
    }
    let mut invoice_pdf_attached = None;
    let outcome = async {
        let builder = Message::builder()
            .from(s.config.email_from.parse()?)
            .to(message.recipient.parse()?)
            .message_id(Some(format!(
                "<vysker-rental-{}@{}>",
                message.id,
                url::Url::parse(&s.config.public_url)?
                    .host_str()
                    .unwrap_or("localhost")
            )))
            .subject(message.subject);
        let mut body = message
            .body
            .ok_or_else(|| anyhow::anyhow!("Missing rental message body"))?;
        let email = if message.purpose == "invoice" {
            let r = row(&mut tx, message.rental_id)
                .await
                .map_err(|_| anyhow::anyhow!("Missing rental"))?;
            let iban = r.account_iban.as_deref().unwrap_or("");
            let total = r
                .total_price
                .ok_or_else(|| anyhow::anyhow!("Missing invoice total"))?;
            // Old queued text may contain untrusted line breaks. Rebuild it
            // from structured fields before formatting it as HTML or PDF.
            if r.invoice_issued_on.is_none() {
                let issued = today(OffsetDateTime::from_unix_timestamp(message.created_at)?);
                let days = r
                    .due_days
                    .ok_or_else(|| anyhow::anyhow!("Missing invoice due date"))?;
                let (account_number, bank_code) =
                    if iban.starts_with("CZ") && iban.len() == 24 && iban.is_ascii() {
                        let domestic = format!("{}-{}", &iban[8..14], &iban[14..24]);
                        super::rental_payment_settings::czech_account(&domestic, &iban[4..8])
                            .map(|(account, bank, _)| (account, bank))
                            .unwrap_or_default()
                    } else {
                        (String::new(), String::new())
                    };
                body = super::rental_invoice_mail::render_text(
                    &super::rental_invoice_mail::InvoiceDetails {
                        id: r.id,
                        asset_name: &r.asset_name,
                        renter_name: &r.name,
                        renter_address: &r.address,
                        organization_ico: r.organization_ico.as_deref(),
                        responsible_name: r.responsible_name.as_deref(),
                        start_date: &r.start_date,
                        end_date: &r.end_date,
                        start_time: r.start_time.as_deref(),
                        end_time: r.end_time.as_deref(),
                        rental_price: r.rental_price,
                        energy_kwh: r.energy_kwh.as_deref().unwrap_or("0"),
                        energy_cost: r.energy_cost.unwrap_or(0),
                        extra_cost: r.extra_cost.unwrap_or(0),
                        total,
                        account_number: &account_number,
                        bank_code: &bank_code,
                        iban,
                        issued_on: issued,
                        due_date: issued + Duration::days(i64::from(days)),
                    },
                );
            }
            let png = if total > 0 {
                Some(
                    super::rental_payment::qr_png(iban, total, message.rental_id)
                        .map_err(anyhow::Error::msg)?,
                )
            } else {
                None
            };
            let pdf = match super::rental_invoice_pdf::render(&body) {
                Ok(pdf) => Some(pdf),
                Err(_) => {
                    tracing::warn!(
                        message_id = message.id,
                        "PDF vyúčtování se nepodařilo vytvořit, odešle se textová verze"
                    );
                    None
                }
            };
            invoice_pdf_attached = Some(pdf.is_some());
            builder.multipart(
                super::rental_invoice_mail::multipart(
                    body,
                    png,
                    message.rental_id,
                    pdf,
                    message.invoice_attachment,
                )
                .map_err(anyhow::Error::msg)?,
            )?
        } else {
            builder
                .header(lettre::message::header::ContentType::TEXT_PLAIN)
                .body(body)?
        };
        smtp.send(email).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if outcome.is_ok() {
        let sent_at = OffsetDateTime::now_utc();
        let updated=sqlx::query("UPDATE rental_mail_queue SET sent_at=$1,body=NULL,invoice_attachment=NULL,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3").bind(sent_at.unix_timestamp()).bind(message.id).bind(&lease).execute(&mut *tx).await?.rows_affected();
        if updated > 0 && message.purpose == "invoice" {
            sqlx::query(
                "UPDATE rentals SET invoice_sent_at=$1,invoice_pdf_attached=$3,status=CASE WHEN NOT wait_for_payment AND status='invoiced' THEN 'closed' ELSE status END,identity=CASE WHEN NOT wait_for_payment THEN NULL ELSE identity END,identity_type=CASE WHEN NOT wait_for_payment THEN NULL ELSE identity_type END WHERE id=$2",
            )
            .bind(timestamp(sent_at))
            .bind(message.rental_id)
            .bind(invoice_pdf_attached)
            .execute(&mut *tx)
            .await?;
            let closed: bool =
                sqlx::query_scalar("SELECT status='closed' FROM rentals WHERE id=$1")
                    .bind(message.rental_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if closed {
                sqlx::query("DELETE FROM rental_access_tokens WHERE rental_id=$1")
                    .bind(message.rental_id)
                    .execute(&mut *tx)
                    .await?;
                audit(
                    &mut tx,
                    None,
                    "closed_without_payment_wait",
                    "rental",
                    message.rental_id,
                    sent_at,
                )
                .await
                .map_err(|_| anyhow::anyhow!("Unable to audit rental closing"))?;
            }
        }
    } else {
        let delay = (30_i64 * 2_i64.pow(message.attempts.min(10) as u32)).min(21600);
        sqlx::query("UPDATE rental_mail_queue SET next_attempt_at=$1,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3").bind(now.unix_timestamp()+delay).bind(message.id).bind(&lease).execute(&mut *tx).await?;
        tracing::warn!(
            message_id = message.id,
            "Rental SMTP failed, retry scheduled"
        );
    }
    tx.commit().await?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;

    struct HeaderOnlyDecoder {
        width: u32,
        height: u32,
        color: image::ColorType,
    }
    impl image::ImageDecoder for HeaderOnlyDecoder {
        fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }
        fn color_type(&self) -> image::ColorType {
            self.color
        }
        fn read_image(self, _: &mut [u8]) -> image::ImageResult<()> {
            panic!("Header validation must not decode or allocate image pixels")
        }
        fn read_image_boxed(self: Box<Self>, bytes: &mut [u8]) -> image::ImageResult<()> {
            (*self).read_image(bytes)
        }
    }

    #[test]
    fn image_header_budget_rejects_high_bit_depth_bombs_but_allows_camera_photos() {
        let check = |width, height, color| {
            check_decoded_image_limits(&HeaderOnlyDecoder {
                width,
                height,
                color,
            })
        };
        // These tests inspect only dimensions and color depth, without allocating pixels.
        assert!(check(4000, 3000, image::ColorType::Rgb8).is_ok());
        assert!(check(6000, 4000, image::ColorType::Rgb8).is_ok());
        assert!(check(6000, 4000, image::ColorType::Rgba8).is_ok());
        assert!(check(4096, 3072, image::ColorType::Rgba16).is_ok()); // Exactly 96 MiB.
        assert!(check(4096, 3073, image::ColorType::Rgba16).is_err());
        assert!(check(8000, 7500, image::ColorType::Rgba16).is_err()); // 480 MB at 60 MP.
        assert!(check(12_001, 1, image::ColorType::L8).is_err());
        assert!(check(10_000, 6001, image::ColorType::L8).is_err());
        assert!(check(0, 1, image::ColorType::L8).is_err());
        let limits = image_limits();
        assert_eq!(limits.max_alloc, Some(96 * 1024 * 1024));
        assert_eq!(limits.max_image_width, Some(12_000));
        assert_eq!(limits.max_image_height, Some(12_000));
    }

    #[test]
    fn sanitized_photo_is_bounded_rotated_and_has_no_exif() {
        use image::ImageEncoder;
        // A narrow real PNG exercises resize and rotation using only 18 KB of pixels.
        let mut png = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut png);
        let exif = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0,
            0,
        ];
        encoder.set_exif_metadata(exif).unwrap();
        encoder
            .write_image(
                &vec![127; 3000 * 2 * 3],
                3000,
                2,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        assert_eq!(check_image(&png).unwrap(), "image/png");
        let output = sanitize_meter_photo(&png).unwrap();
        let decoded = image::load_from_memory(&output).unwrap();
        assert_eq!(decoded.height(), IMAGE_OUTPUT_DIMENSION);
        assert!(decoded.width() <= 2);
        assert!(
            exif::Reader::new()
                .read_from_container(&mut std::io::Cursor::new(output))
                .is_err()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelling_an_upload_keeps_its_image_slot_until_the_blocking_work_finishes() {
        static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let slot = SLOTS.acquire().await.unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let request = tokio::spawn(process_image_files(slot, move || {
            started_tx.send(()).unwrap();
            finish_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Ok(())
        }));
        tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert!(
            SLOTS.try_acquire().is_err(),
            "Cancelled request released a running decoder's slot"
        );
        finish_tx.send(()).unwrap();
        let released = tokio::time::timeout(std::time::Duration::from_secs(2), SLOTS.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(released);
    }

    #[test]
    fn rental_start_must_stay_strictly_in_the_future_at_every_check() {
        use time::macros::datetime;
        let cases = [
            ("2026-11-02", None, None, datetime!(2026-11-01 23:00 UTC)),
            (
                "2026-11-02",
                Some("10:00"),
                Some("11:00"),
                datetime!(2026-11-02 09:00 UTC),
            ),
            // Prague's civil day starts before the UTC date and changes length around DST.
            ("2026-03-29", None, None, datetime!(2026-03-28 23:00 UTC)),
            ("2026-10-25", None, None, datetime!(2026-10-24 22:00 UTC)),
        ];
        for (date, from, to, boundary) in cases {
            let period = Period::new(date, date, from, to, true).unwrap();
            assert_eq!(period.start_instant().unwrap(), boundary);
            assert!(
                period
                    .require_future_start(boundary - Duration::nanoseconds(1))
                    .is_ok()
            );
            assert!(period.require_future_start(boundary).is_err());
            // An earlier successful check must not authorize a request after waiting or processing.
            assert!(
                period
                    .require_future_start(boundary + Duration::nanoseconds(1))
                    .is_err()
            );
        }
        let spring = Period::new("2026-03-29", "2026-03-29", None, None, true).unwrap();
        assert_eq!(
            (spring.end_instant().unwrap() - spring.start_instant().unwrap()).whole_hours(),
            23
        );
        let autumn = Period::new("2026-10-25", "2026-10-25", None, None, true).unwrap();
        assert_eq!(
            (autumn.end_instant().unwrap() - autumn.start_instant().unwrap()).whole_hours(),
            25
        );
    }

    #[test]
    fn admin_periods_reject_started_first_occurrence_for_each_recurrence_mode() {
        use time::macros::datetime;
        for (from, to, boundary) in [
            (None, None, datetime!(2026-11-01 23:00 UTC)),
            (
                Some("10:00"),
                Some("11:00"),
                datetime!(2026-11-02 09:00 UTC),
            ),
        ] {
            for (days, months) in [(0, 0), (7, 0), (14, 0), (0, 1)] {
                let input: AdminRentalInput = serde_json::from_value(serde_json::json!({
                    "asset_id":1,"start_date":"2026-11-02","end_date":"2026-11-02",
                    "start_time":from,"end_time":to,"name":"Test","email":"test@example.test",
                    "phone":"123456789","address":"Test 1","event_type":"standard",
                    "rental_price":0,"require_meter":false,"wait_for_payment":false,
                    "recurrence_interval_days":days,"recurrence_interval_months":months,
                    "recurrence_until": if days != 0 || months != 0 {Some("2026-12-31")} else {None}
                }))
                .unwrap();
                assert!(input.periods(boundary - Duration::nanoseconds(1)).is_ok());
                assert!(input.periods(boundary).is_err());
                assert!(input.periods(boundary + Duration::hours(1)).is_err());
            }
        }
    }

    #[test]
    fn monthly_weekday_preserves_ordinal_across_leap_months_and_years() {
        use time::macros::date;
        assert_eq!(
            monthly_weekday_starts(date!(2024 - 02 - 29), date!(2025 - 01 - 30)).unwrap(),
            vec![
                date!(2024 - 02 - 29),
                date!(2024 - 05 - 30),
                date!(2024 - 08 - 29),
                date!(2024 - 10 - 31),
                date!(2025 - 01 - 30)
            ]
        );
        assert_eq!(
            monthly_weekday_starts(date!(2026 - 12 - 10), date!(2027 - 03 - 11)).unwrap(),
            vec![
                date!(2026 - 12 - 10),
                date!(2027 - 01 - 14),
                date!(2027 - 02 - 11),
                date!(2027 - 03 - 11)
            ]
        );
        assert_eq!(
            monthly_weekday_starts(date!(2026 - 12 - 10), date!(2027 - 01 - 13)).unwrap(),
            vec![date!(2026 - 12 - 10)]
        );
    }

    fn jpeg_with_exif_time(value: &[u8; 20]) -> Vec<u8> {
        let mut jpeg = Vec::new();
        let pixel = image::RgbImage::from_pixel(1, 1, image::Rgb([2, 3, 4]));
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&pixel)
            .unwrap();
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0".to_vec();
        // IFD0 points to the Exif IFD at byte 26 of the TIFF payload.
        exif.extend_from_slice(&[1, 0, 0x69, 0x87, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0]);
        // DateTimeOriginal is a 20-byte ASCII string at TIFF offset 44.
        exif.extend_from_slice(&[1, 0, 0x03, 0x90, 2, 0, 20, 0, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0]);
        exif.extend_from_slice(value);
        let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
        out.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&exif);
        out.extend_from_slice(&jpeg[2..]);
        out
    }
    #[test]
    fn extracts_only_valid_camera_time_and_strips_metadata() {
        let original = jpeg_with_exif_time(b"2026:10:30 12:34:56\0");
        assert_eq!(
            photo_taken_at(&original).as_deref(),
            Some("2026-10-30 12:34:56")
        );
        let sanitized = sanitize_meter_photo(&original).unwrap();
        assert_eq!(photo_taken_at(&sanitized), None);
        assert!(
            exif::Reader::new()
                .read_from_container(&mut std::io::Cursor::new(sanitized))
                .is_err()
        );
        assert_eq!(
            photo_taken_at(&jpeg_with_exif_time(b"2026:02:30 12:34:56\0")),
            None
        );
        assert_eq!(photo_taken_at(b"not a photo"), None);
    }
    #[test]
    fn whole_days_are_available_all_week_and_hours_only_on_weekdays() {
        let day = |start, end| Period::new(start, end, None, None, true);
        assert!(day("2026-10-30", "2026-10-30").is_ok());
        assert!(day("2026-10-31", "2026-10-31").is_ok());
        assert!(day("2026-11-01", "2026-11-01").is_ok());
        assert!(day("2026-10-31", "2026-11-01").is_ok());
        assert!(day("2026-10-30", "2026-11-01").is_ok());
        assert!(day("2026-11-01", "2026-11-02").is_ok());
        assert!(day("2026-10-31", "2026-11-08").is_ok());
        assert!(
            Period::new(
                "2026-10-30",
                "2026-10-30",
                Some("22:00"),
                Some("24:00"),
                true
            )
            .is_ok()
        );
        assert!(
            Period::new(
                "2026-10-30",
                "2026-10-30",
                Some("00:00"),
                Some("24:00"),
                true
            )
            .is_ok()
        );
        // Maintenance blocks and historical agreements keep the original period.
        assert!(Period::new("2026-10-30", "2026-11-15", None, None, false).is_ok());
        assert!(
            Period::new(
                "2026-10-31",
                "2026-10-31",
                Some("22:00"),
                Some("24:00"),
                false
            )
            .is_ok()
        );
        assert!(
            Period::new(
                "2026-10-31",
                "2026-10-31",
                Some("22:00"),
                Some("24:00"),
                true
            )
            .is_err()
        );
        assert!(
            Period::new(
                "2026-10-30",
                "2026-10-30",
                Some("22:30"),
                Some("24:00"),
                true
            )
            .is_err()
        );
        assert!(Period::new("2026-10-30", "2026-10-30", Some("22:00"), None, true).is_err());
        assert!(
            Period::new(
                "2026-10-30",
                "2026-10-30",
                Some("22:00"),
                Some("22:00"),
                true
            )
            .is_err()
        );
        assert!(day("2026-11-02", "2026-11-29").is_ok());
        assert!(day("2026-11-02", "2026-11-30").is_err());
        assert!(Period::new("2026-10-30", "2026-10-30", Some("€00"), Some("24:00"), true).is_err());
    }
}
