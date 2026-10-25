//! Public fault reports with private request metadata and durable notifications.
use super::{
    Backend, Error, Pagination, Result, audit, auth, bad, missing, report_abuse, timestamp,
};
use crate::reports::{
    AdminReport, REPORT_ADDRESS_LIMIT, REPORT_DESCRIPTION_LIMIT, REPORT_MAP_LIMIT,
    REPORT_NAME_LIMIT, REPORT_PHOTO_BYTES, REPORT_RESOLUTION_LIMIT, REPORT_SEARCH_LIMIT,
    REPORT_UPLOAD_BYTES, Report, ReportMap, ReportMapPoint, ReportSettings, ReportStatusFilter,
    ReportUpdate, report_excerpt,
};
use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use sqlx::PgPool;
use std::{collections::BTreeSet, io::Cursor};
use time::OffsetDateTime;
use time_tz::OffsetDateTimeExt;

const RECIPIENT_LIMIT: usize = 20;
static IMAGE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

fn optional_text(value: String, max: usize, multiline: bool) -> Result<Option<String>> {
    let value = value.replace("\r\n", "\n");
    let value = value.trim();
    if value.chars().count() > max
        || value
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\t')))
    {
        return Err(bad("Text je příliš dlouhý nebo obsahuje nepovolené znaky."));
    }
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn reporter_email(value: String) -> Result<Option<String>> {
    optional_text(value, 254, false)?
        .map(|value| auth::email(&value))
        .transpose()
}

fn reporter_phone(value: String) -> Result<Option<String>> {
    let Some(value) = optional_text(value, 40, false)? else {
        return Ok(None);
    };
    let digits = value.chars().filter(char::is_ascii_digit).count();
    if !(3..=20).contains(&digits)
        || value.chars().filter(|c| *c == '+').count() > 1
        || !value
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_whitespace() || "+()-./".contains(c))
    {
        return Err(bad(
            "Zadejte platné telefonní číslo, například +420 123 456 789.",
        ));
    }
    Ok(Some(value.split_whitespace().collect::<Vec<_>>().join(" ")))
}

fn coordinate(value: Option<String>, min: f64, max: f64) -> Result<Option<f64>> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            let number: f64 = value.trim().parse().map_err(|_| bad("Neplatná poloha."))?;
            if !number.is_finite() || !(min..=max).contains(&number) {
                return Err(bad("Neplatná poloha."));
            }
            Ok(number)
        })
        .transpose()
}

/// A plain numeric pair is a location, while ordinary street/place text remains
/// an address. Decimal commas are unambiguous when separated by a semicolon.
fn address_coordinates(address: &str) -> Result<Option<(f64, f64)>> {
    let decimal_comma = address.contains(';');
    let parts: Vec<&str> = if decimal_comma {
        address.split(';').collect()
    } else if address.contains(',') {
        address.split(',').collect()
    } else {
        address.split_whitespace().collect()
    };
    if parts.len() != 2 {
        return Ok(None);
    }
    let number = |value: &str| -> Option<f64> {
        let value = value.trim();
        let value = if decimal_comma {
            value.replace(',', ".")
        } else {
            value.to_owned()
        };
        let unsigned = value.strip_prefix(['+', '-']).unwrap_or(&value);
        if !unsigned.chars().any(|c| c.is_ascii_digit())
            || !unsigned.chars().all(|c| c.is_ascii_digit() || c == '.')
            || unsigned.matches('.').count() > 1
        {
            return None;
        }
        value.parse().ok()
    };
    let (Some(latitude), Some(longitude)) = (number(parts[0]), number(parts[1])) else {
        return Ok(None);
    };
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return Err(bad(
            "Neplatné GPS souřadnice. Zeměpisná šířka musí být od -90 do 90 a délka od -180 do 180.",
        ));
    }
    Ok(Some((latitude, longitude)))
}

/// Decode pixels, preserve their intended orientation and create a fresh JPEG.
/// No original EXIF, GPS, embedded profiles or other metadata reach the public file.
fn sanitize_photo(bytes: &[u8]) -> Result<Vec<u8>> {
    let format = image::guess_format(bytes)
        .map_err(|_| bad("Fotografie musí být ve formátu JPEG, PNG nebo WebP."))?;
    if !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
    ) {
        return Err(bad("Fotografie musí být ve formátu JPEG, PNG nebo WebP."));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let invalid = || {
        bad(
            "Fotografie je poškozená nebo příliš velká. Nejvýše 8192 bodů na straně a 32 milionů bodů celkem.",
        )
    };
    let mut decoder = reader.into_decoder().map_err(|_| invalid())?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > 32_000_000
        || decoder.total_bytes() > 128 * 1024 * 1024
    {
        return Err(invalid());
    }
    let orientation = decoder.orientation().map_err(|_| invalid())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| invalid())?;
    image.apply_orientation(orientation);
    if image.width() > 2560 || image.height() > 2560 {
        image = image.thumbnail(2560, 2560);
    }
    let mut result = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut result, 85)
        .encode_image(&image.to_rgb8())
        .map_err(|_| bad("Fotografii se nepodařilo zpracovat."))?;
    if result.len() > REPORT_PHOTO_BYTES {
        return Err(bad("Zpracovaná fotografie překračuje limit 15 MB."));
    }
    Ok(result)
}

async fn read_photo(
    field: &mut axum::extract::multipart::Field<'_>,
    upload_bytes: &mut usize,
    processed_bytes: &mut usize,
) -> Result<Option<Vec<u8>>> {
    let empty_file = field.file_name().is_none_or(str::is_empty);
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::time::timeout(std::time::Duration::from_secs(60), field.chunk())
        .await
        .map_err(|_| bad("Přenos dat se přerušil. Zkuste odeslání znovu."))?
        .map_err(|_| bad("Fotografie nelze přečíst nebo celková velikost překračuje 50 MB."))?
    {
        if chunk.len() > REPORT_PHOTO_BYTES - bytes.len() {
            return Err(bad("Jedna fotografie může mít nejvýše 15 MB."));
        }
        if chunk.len() > REPORT_UPLOAD_BYTES - *upload_bytes {
            return Err(bad("Fotografie mohou mít dohromady nejvýše 50 MB."));
        }
        *upload_bytes += chunk.len();
        bytes.extend_from_slice(&chunk);
    }
    if empty_file && bytes.is_empty() {
        return Ok(None);
    }
    if bytes.is_empty() {
        return Err(bad("Fotografie je prázdná."));
    }
    let permit = IMAGE_SLOTS
        .acquire()
        .await
        .map_err(|_| bad("Fotografii nyní nelze zpracovat."))?;
    let photo = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        sanitize_photo(&bytes)
    })
    .await
    .map_err(|_| bad("Fotografii se nepodařilo zpracovat."))??;
    if photo.len() > REPORT_UPLOAD_BYTES - *processed_bytes {
        return Err(bad(
            "Zpracované fotografie mohou mít dohromady nejvýše 50 MB. Přiložte méně fotografií.",
        ));
    }
    *processed_bytes += photo.len();
    Ok(Some(photo))
}

async fn read_text_field(
    field: &mut axum::extract::multipart::Field<'_>,
    limit: usize,
) -> Result<String> {
    let mut value = Vec::new();
    while let Some(chunk) = tokio::time::timeout(std::time::Duration::from_secs(60), field.chunk())
        .await
        .map_err(|_| bad("Přenos dat se přerušil. Zkuste odeslání znovu."))?
        .map_err(|_| bad("Pole hlášení nelze přečíst."))?
    {
        if chunk.len() > limit * 4 - value.len() {
            return Err(bad("Pole hlášení je příliš dlouhé."));
        }
        value.extend_from_slice(&chunk);
    }
    String::from_utf8(value).map_err(|_| bad("Neplatný text hlášení."))
}

struct SubmittedReport {
    submission_token: Option<String>,
    client_timezone: Option<String>,
    description: Option<String>,
    address: Option<String>,
    name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    latitude: Option<String>,
    longitude: Option<String>,
    photos: Vec<Vec<u8>>,
}

async fn read_submission(
    multipart: &mut Multipart,
    s: &Backend,
    context: &report_abuse::SubmissionContext,
) -> report_abuse::SubmissionResult<SubmittedReport> {
    let (mut submission_token, mut client_timezone) = (None, None);
    let mut seen = BTreeSet::new();
    let (mut description, mut address, mut name, mut latitude, mut longitude) =
        (None, None, None, None, None);
    let (mut email, mut phone) = (None, None);
    let mut photos = Vec::new();
    let mut upload_bytes = 0;
    let mut processed_bytes = 0;
    while let Some(mut field) =
        tokio::time::timeout(std::time::Duration::from_secs(60), multipart.next_field())
            .await
            .map_err(|_| bad("Přenos dat se přerušil. Zkuste odeslání znovu."))?
            .map_err(|_| bad("Hlášení nelze přečíst nebo překračuje povolenou velikost."))?
    {
        let key = field.name().unwrap_or("").to_owned();
        if key == "photo" {
            let token = submission_token
                .as_deref()
                .ok_or(report_abuse::SubmissionError::InvalidToken)?;
            report_abuse::token_valid(
                &s.pool,
                &context,
                token,
                OffsetDateTime::now_utc().unix_timestamp(),
            )
            .await?;
            if let Some(photo) =
                read_photo(&mut field, &mut upload_bytes, &mut processed_bytes).await?
            {
                photos.push(photo);
            }
            continue;
        }
        if !seen.insert(key.clone()) {
            return Err(bad("Každé textové pole může být v hlášení jen jednou.").into());
        }
        let limit = match key.as_str() {
            "description" => REPORT_DESCRIPTION_LIMIT,
            "address" => REPORT_ADDRESS_LIMIT,
            "reporter_name" => REPORT_NAME_LIMIT,
            "reporter_email" => 254,
            "reporter_phone" => 40,
            "latitude" | "longitude" => 32,
            "submission_token" => 64,
            "client_timezone" => 128,
            "website" => 300,
            _ => return Err(bad("Hlášení obsahuje neznámé pole.").into()),
        };
        let value = read_text_field(&mut field, limit).await?;
        match key.as_str() {
            "description" => description = optional_text(value, limit, true)?,
            "address" => address = optional_text(value, limit, false)?,
            "reporter_name" => name = optional_text(value, limit, false)?,
            "reporter_email" => email = reporter_email(value)?,
            "reporter_phone" => phone = reporter_phone(value)?,
            "latitude" => latitude = Some(value),
            "longitude" => longitude = Some(value),
            "submission_token" => {
                report_abuse::token_valid(
                    &s.pool,
                    &context,
                    &value,
                    OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await?;
                submission_token = Some(value);
            }
            "client_timezone" => client_timezone = report_abuse::timezone(&value)?,
            "website" => {
                if !value.trim().is_empty() {
                    return Err(bad("Hlášení bylo vyhodnoceno jako automatické vyplnění. Nechte pomocné pole prázdné.").into());
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(SubmittedReport {
        submission_token,
        client_timezone,
        description,
        address,
        name,
        email,
        phone,
        latitude,
        longitude,
        photos,
    })
}

pub async fn create(
    State(s): State<Backend>,
    auth::ClientIp(ip): auth::ClientIp,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> report_abuse::SubmissionResult<(StatusCode, Json<Report>)> {
    let now = OffsetDateTime::now_utc();
    let context = report_abuse::begin(&s, &headers, &ip, now.unix_timestamp()).await?;
    // Acquire before reading any uploads. A bounded wait prevents a request backlog.
    let _submission_permit = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        report_abuse::SUBMISSION_SLOTS.acquire(),
    )
    .await
    .map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Právě se zpracovávají jiná hlášení. Zkuste odeslání za chvíli.",
        )
    })?
    .map_err(|_| bad("Hlášení nyní nelze zpracovat."))?;
    let SubmittedReport {
        submission_token,
        client_timezone,
        description,
        mut address,
        name,
        email,
        phone,
        latitude,
        longitude,
        photos,
    } = tokio::time::timeout(
        std::time::Duration::from_secs(15 * 60),
        read_submission(&mut multipart, &s, &context),
    )
    .await
    .map_err(|_| bad("Odesílání trvalo příliš dlouho. Zkuste ho prosím znovu."))??;
    let description = description.ok_or_else(|| bad("Popište prosím podnět nebo závadu."))?;
    let mut latitude = coordinate(latitude, -90., 90.)?;
    let mut longitude = coordinate(longitude, -180., 180.)?;
    if latitude.is_some() != longitude.is_some() {
        return Err(bad("Poloha musí obsahovat obě souřadnice.").into());
    }
    if let Some((address_latitude, address_longitude)) = address
        .as_deref()
        .map(address_coordinates)
        .transpose()?
        .flatten()
    {
        let differs = |explicit: f64, from_address: f64| {
            // Account for binary floating-point rounding at the tolerance edge.
            (explicit - from_address).abs() > 1e-6 + f64::EPSILON * from_address.abs().max(1.0)
        };
        if latitude.is_some_and(|value| differs(value, address_latitude))
            || longitude.is_some_and(|value| differs(value, address_longitude))
        {
            return Err(bad("GPS souřadnice v adrese se neshodují s vybranou polohou.").into());
        }
        latitude = Some(address_latitude);
        longitude = Some(address_longitude);
        address = Some(format!("{address_latitude:.6}, {address_longitude:.6}"));
    }
    if address.is_none() && latitude.is_none() {
        return Err(bad("Vyplňte adresu nebo vyberte místo na mapě.").into());
    }
    let now = OffsetDateTime::now_utc();
    let token = submission_token.ok_or(report_abuse::SubmissionError::InvalidToken)?;
    let user_agent = report_abuse::bounded_header(&headers, "user-agent", 512);
    let language = report_abuse::bounded_header(&headers, "accept-language", 256);
    let fingerprint = report_abuse::fingerprint(
        &context,
        user_agent.as_deref(),
        language.as_deref(),
        client_timezone.as_deref(),
    );
    let authenticated =
        report_abuse::authenticated_sender(&s.pool, &headers, now.unix_timestamp()).await?;
    let photo_hashes: Vec<String> = photos.iter().map(auth::hash).collect();
    let payload = auth::hash(
        serde_json::json!([
            description,
            address,
            latitude,
            longitude,
            name,
            email,
            phone,
            photo_hashes
        ])
        .to_string(),
    );
    let mut tx = crate::db::begin_write(&s.pool).await?;
    if let Some(id) =
        report_abuse::claim(&mut tx, &context, &token, &payload, now.unix_timestamp()).await?
    {
        let report = sqlx::query_as(PUBLIC_REPORT_BY_ID)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                super::conflict("Toto hlášení už bylo přijato a nyní není veřejně dostupné.")
            })?;
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(report)));
    }
    let report: Report = sqlx::query_as("INSERT INTO issue_reports(description,address,latitude,longitude,reporter_name,request_ip,created_at,request_user_agent,request_accept_language,client_timezone,browser_id_hash,browser_fingerprint_hash,authenticated_admin_id,authenticated_admin_email,metadata_recorded_at,metadata_expires_at,reporter_email,reporter_phone) VALUES ($1,$2,$3,$4,$5,$6,$7,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19) RETURNING id,description,address,latitude,longitude,created_at,status,resolution_comment,resolution_updated_at,ARRAY(SELECT id FROM issue_report_resolution_photos WHERE report_id=issue_reports.id ORDER BY id) AS resolution_photo_ids,$8::bigint > 0 AS has_photo,$8::bigint AS photo_count")
        .bind(description).bind(address).bind(latitude).bind(longitude).bind(name).bind(ip)
        .bind(timestamp(now.to_timezone(time_tz::timezones::db::europe::PRAGUE))).bind(photos.len() as i64)
        .bind(user_agent).bind(language).bind(client_timezone).bind(context.persistent_browser.then_some(&context.browser_hash)).bind(fingerprint)
        .bind(authenticated.as_ref().map(|(id,_)| *id)).bind(authenticated.as_ref().map(|(_,email)| email.as_str())).bind(now.unix_timestamp()).bind(now.unix_timestamp()+30*86400).bind(email).bind(phone)
        .fetch_one(&mut *tx).await?;
    report_abuse::record(
        &mut tx,
        &context,
        &token,
        &payload,
        report.id,
        now.unix_timestamp(),
    )
    .await?;
    for (position, photo) in photos.into_iter().enumerate() {
        sqlx::query("INSERT INTO issue_report_photos(report_id,position,photo) VALUES ($1,$2,$3)")
            .bind(report.id)
            .bind(position as i64 + 1)
            .bind(photo)
            .execute(&mut *tx)
            .await?;
    }
    let recipients: Vec<String> =
        sqlx::query_scalar("SELECT recipients FROM report_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
    let body = notification_body(&s, &report);
    for recipient in recipients {
        sqlx::query("INSERT INTO report_mail_queue(report_id,recipient,subject,body,next_attempt_at,created_at) VALUES ($1,$2,$3,$4,$5,$5)")
            .bind(report.id).bind(recipient).bind(format!("Vyskeř: nový podnět nebo závada č. {}", report.id))
            .bind(&body).bind(now.unix_timestamp()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(report)))
}

fn notification_body(s: &Backend, report: &Report) -> String {
    let mut body = format!(
        "Nový podnět nebo závada č. {}\nNahlášeno: {}\n\n{}\n\n",
        report.id,
        crate::events::date_label(&report.created_at),
        report.description
    );
    if let Some(address) = &report.address {
        body.push_str(&format!("Adresa: {address}\n"));
    }
    if let (Some(latitude), Some(longitude)) = (report.latitude, report.longitude) {
        body.push_str(&format!("Poloha: {latitude}, {longitude}\nMapa: https://www.openstreetmap.org/?mlat={latitude}&mlon={longitude}#map=18/{latitude}/{longitude}\n"));
    }
    body.push_str(&format!(
        "\nDetail hlášení: {}{}\n",
        s.config.public_url,
        report.href()
    ));
    for (index, photo_url) in report.photo_urls().into_iter().enumerate() {
        body.push_str(&format!(
            "Fotografie {}: {}{}\n",
            index + 1,
            s.config.public_url,
            photo_url
        ));
    }
    body
}

pub async fn public_reports_from_pool(
    pool: &PgPool,
    limit: i64,
    offset: i64,
    q: &str,
    status: ReportStatusFilter,
) -> Result<Vec<Report>> {
    let (limit, offset) = Pagination {
        limit: Some(limit),
        offset: Some(offset),
    }
    .bounds()?;
    let pattern = search_pattern(q)?;
    Ok(sqlx::query_as("SELECT id,description,address,latitude,longitude,created_at,status,resolution_comment,resolution_updated_at,ARRAY(SELECT id FROM issue_report_resolution_photos WHERE report_id=issue_reports.id ORDER BY id) AS resolution_photo_ids,EXISTS(SELECT 1 FROM issue_report_photos WHERE report_id=issue_reports.id) AS has_photo,(SELECT count(*) FROM issue_report_photos WHERE report_id=issue_reports.id) AS photo_count FROM issue_reports WHERE hidden=FALSE AND (description ILIKE $3 ESCAPE '!' OR address ILIKE $3 ESCAPE '!') AND status=ANY($4) ORDER BY id DESC LIMIT $1 OFFSET $2")
        .bind(limit).bind(offset).bind(pattern).bind(status.statuses()).fetch_all(pool).await?)
}

fn search_pattern(q: &str) -> Result<String> {
    let query = optional_text(q.to_owned(), REPORT_SEARCH_LIMIT, false)?.unwrap_or_default();
    Ok(format!(
        "%{}%",
        query
            .replace('!', "!!")
            .replace('%', "!%")
            .replace('_', "!_")
    ))
}

pub async fn public_report_map_from_pool(
    pool: &PgPool,
    q: &str,
    status: ReportStatusFilter,
) -> Result<ReportMap> {
    let pattern = search_pattern(q)?;
    // Counts and points share one snapshot even when new reports arrive or an
    // administrator changes visibility while the map is loading.
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let (located_count, unlocated_count): (i64, i64) = sqlx::query_as("SELECT count(*) FILTER (WHERE latitude IS NOT NULL),count(*) FILTER (WHERE latitude IS NULL) FROM issue_reports WHERE hidden=FALSE AND (description ILIKE $1 ESCAPE '!' OR address ILIKE $1 ESCAPE '!') AND status=ANY($2)")
        .bind(&pattern).bind(status.statuses()).fetch_one(&mut *tx).await?;
    let mut points: Vec<ReportMapPoint> = sqlx::query_as("SELECT id,latitude,longitude,description,address,created_at,status,EXISTS(SELECT 1 FROM issue_report_photos WHERE report_id=issue_reports.id) AS has_photo FROM issue_reports WHERE hidden=FALSE AND latitude IS NOT NULL AND (description ILIKE $1 ESCAPE '!' OR address ILIKE $1 ESCAPE '!') AND status=ANY($3) ORDER BY id DESC LIMIT $2")
        .bind(&pattern).bind(REPORT_MAP_LIMIT).bind(status.statuses()).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    for point in &mut points {
        point.description = report_excerpt(&point.description);
    }
    Ok(ReportMap {
        truncated: located_count > points.len() as i64,
        points,
        located_count,
        unlocated_count,
    })
}

const PUBLIC_REPORT_BY_ID: &str = "SELECT id,description,address,latitude,longitude,created_at,status,resolution_comment,resolution_updated_at,ARRAY(SELECT id FROM issue_report_resolution_photos WHERE report_id=issue_reports.id ORDER BY id) AS resolution_photo_ids,EXISTS(SELECT 1 FROM issue_report_photos WHERE report_id=issue_reports.id) AS has_photo,(SELECT count(*) FROM issue_report_photos WHERE report_id=issue_reports.id) AS photo_count FROM issue_reports WHERE id=$1 AND hidden=FALSE";

pub async fn public_report_from_pool(pool: &PgPool, id: i64) -> Result<Option<Report>> {
    Ok(sqlx::query_as(PUBLIC_REPORT_BY_ID)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

#[derive(serde::Deserialize, Default)]
pub struct PublicQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: ReportStatusFilter,
}

pub async fn public_list(
    State(s): State<Backend>,
    Query(query): Query<PublicQuery>,
) -> Result<Json<Vec<Report>>> {
    let (limit, offset) = Pagination {
        limit: query.limit,
        offset: query.offset,
    }
    .bounds()?;
    Ok(Json(
        public_reports_from_pool(&s.pool, limit, offset, &query.q, query.status).await?,
    ))
}

#[derive(serde::Deserialize, Default)]
pub struct MapQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: ReportStatusFilter,
}

pub async fn public_map(
    State(s): State<Backend>,
    Query(query): Query<MapQuery>,
) -> Result<Json<ReportMap>> {
    Ok(Json(
        public_report_map_from_pool(&s.pool, &query.q, query.status).await?,
    ))
}

pub async fn detail(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Json<Report>> {
    Ok(Json(
        public_report_from_pool(&s.pool, id)
            .await?
            .ok_or_else(missing)?,
    ))
}

pub async fn list(
    State(s): State<Backend>,
    _: auth::Admin,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<AdminReport>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(sqlx::query_as("SELECT id,description,address,latitude,longitude,created_at,status,resolution_comment,resolution_updated_at,ARRAY(SELECT id FROM issue_report_resolution_photos WHERE report_id=issue_reports.id ORDER BY id) AS resolution_photo_ids,EXISTS(SELECT 1 FROM issue_report_photos WHERE report_id=issue_reports.id) AS has_photo,(SELECT count(*) FROM issue_report_photos WHERE report_id=issue_reports.id) AS photo_count,request_ip,hidden,reporter_name,reporter_email,reporter_phone,request_user_agent,request_accept_language,client_timezone,browser_id_hash,browser_fingerprint_hash,authenticated_admin_id,authenticated_admin_email,metadata_recorded_at,metadata_expires_at FROM issue_reports ORDER BY id DESC LIMIT $1 OFFSET $2")
        .bind(limit).bind(offset).fetch_all(&s.pool).await?))
}

pub async fn update(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    Json(input): Json<ReportUpdate>,
) -> Result<Json<AdminReport>> {
    if input.status.is_none() && input.hidden.is_none() {
        return Err(bad("Vyberte stav nebo viditelnost hlášení."));
    }
    if input
        .status
        .as_deref()
        .is_some_and(|status| !matches!(status, "reported" | "in_progress" | "resolved"))
    {
        return Err(bad("Neplatný stav hlášení."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let report: AdminReport = sqlx::query_as("UPDATE issue_reports SET status=COALESCE($1,status),hidden=COALESCE($2,hidden),resolution_updated_at=CASE WHEN $1 IS NOT NULL AND $1<>status THEN $4 ELSE resolution_updated_at END WHERE id=$3 RETURNING id,description,address,latitude,longitude,created_at,status,resolution_comment,resolution_updated_at,ARRAY(SELECT id FROM issue_report_resolution_photos WHERE report_id=issue_reports.id ORDER BY id) AS resolution_photo_ids,EXISTS(SELECT 1 FROM issue_report_photos WHERE report_id=issue_reports.id) AS has_photo,(SELECT count(*) FROM issue_report_photos WHERE report_id=issue_reports.id) AS photo_count,request_ip,hidden,reporter_name,reporter_email,reporter_phone,request_user_agent,request_accept_language,client_timezone,browser_id_hash,browser_fingerprint_hash,authenticated_admin_id,authenticated_admin_email,metadata_recorded_at,metadata_expires_at")
        .bind(input.status).bind(input.hidden).bind(id).bind(timestamp(OffsetDateTime::now_utc().to_timezone(time_tz::timezones::db::europe::PRAGUE))).fetch_optional(&mut *tx).await?.ok_or_else(missing)?;
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "issue_report",
        id,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(report))
}

const ADMIN_REPORT_BY_ID: &str = "SELECT id,description,address,latitude,longitude,created_at,status,resolution_comment,resolution_updated_at,ARRAY(SELECT id FROM issue_report_resolution_photos WHERE report_id=issue_reports.id ORDER BY id) AS resolution_photo_ids,EXISTS(SELECT 1 FROM issue_report_photos WHERE report_id=issue_reports.id) AS has_photo,(SELECT count(*) FROM issue_report_photos WHERE report_id=issue_reports.id) AS photo_count,request_ip,hidden,reporter_name,reporter_email,reporter_phone,request_user_agent,request_accept_language,client_timezone,browser_id_hash,browser_fingerprint_hash,authenticated_admin_id,authenticated_admin_email,metadata_recorded_at,metadata_expires_at FROM issue_reports WHERE id=$1";

pub async fn update_resolution(
    State(s): State<Backend>,
    admin: auth::Admin,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> Result<Json<AdminReport>> {
    let mut seen = BTreeSet::new();
    let mut status = None;
    let mut comment = None;
    let mut remove_ids = BTreeSet::new();
    let mut photos = Vec::new();
    let mut upload_bytes = 0;
    let mut processed_bytes = 0;
    while let Some(mut field) =
        tokio::time::timeout(std::time::Duration::from_secs(60), multipart.next_field())
            .await
            .map_err(|_| bad("Přenos dat se přerušil. Zkuste odeslání znovu."))?
            .map_err(|_| bad("Řešení nelze přečíst nebo překračuje povolenou velikost."))?
    {
        let key = field.name().unwrap_or("").to_owned();
        if key == "photo" {
            if let Some(photo) =
                read_photo(&mut field, &mut upload_bytes, &mut processed_bytes).await?
            {
                photos.push(photo);
            }
            continue;
        }
        if !seen.insert(key.clone()) {
            return Err(bad("Každé textové pole může být v řešení jen jednou."));
        }
        match key.as_str() {
            "status" => {
                let value = read_text_field(&mut field, 32).await?;
                let value = value.trim();
                if !matches!(value, "reported" | "in_progress" | "resolved") {
                    return Err(bad("Neplatný stav hlášení."));
                }
                status = Some(value.to_owned());
            }
            "resolution_comment" => {
                comment = Some(optional_text(
                    read_text_field(&mut field, REPORT_RESOLUTION_LIMIT).await?,
                    REPORT_RESOLUTION_LIMIT,
                    true,
                )?);
            }
            "remove_photo_ids" => {
                let value = read_text_field(&mut field, 8_192).await?;
                let ids: Vec<i64> = serde_json::from_str(&value)
                    .map_err(|_| bad("Neplatný seznam fotografií k odebrání."))?;
                for photo_id in ids {
                    if photo_id <= 0 || !remove_ids.insert(photo_id) {
                        return Err(bad("Neplatný seznam fotografií k odebrání."));
                    }
                }
            }
            _ => return Err(bad("Řešení obsahuje neznámé pole.")),
        }
    }
    if seen.is_empty() && photos.is_empty() {
        return Err(bad("Vyberte stav, komentář nebo fotografie řešení."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM issue_reports WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    let current: AdminReport = sqlx::query_as(ADMIN_REPORT_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let existing: Vec<(i64, i64)> = sqlx::query_as("SELECT id,octet_length(photo)::bigint FROM issue_report_resolution_photos WHERE report_id=$1")
        .bind(id).fetch_all(&mut *tx).await?;
    if remove_ids
        .iter()
        .any(|photo_id| !existing.iter().any(|(id, _)| id == photo_id))
    {
        return Err(bad(
            "Fotografie k odebrání nepatří k tomuto řešení nebo už byla odstraněna.",
        ));
    }
    let retained_bytes: i64 = existing
        .iter()
        .filter(|(photo_id, _)| !remove_ids.contains(photo_id))
        .map(|(_, bytes)| bytes)
        .sum();
    if retained_bytes > (REPORT_UPLOAD_BYTES - processed_bytes) as i64 {
        return Err(bad(
            "Fotografie řešení mohou mít dohromady nejvýše 50 MB. Odeberte některé stávající fotografie.",
        ));
    }
    let status = status.unwrap_or_else(|| current.report.status.clone());
    let comment = comment.unwrap_or_else(|| current.report.resolution_comment.clone());
    if status == current.report.status
        && comment == current.report.resolution_comment
        && remove_ids.is_empty()
        && photos.is_empty()
    {
        tx.commit().await?;
        return Ok(Json(current));
    }
    let now = OffsetDateTime::now_utc();
    sqlx::query("UPDATE issue_reports SET status=$1,resolution_comment=$2,resolution_updated_at=$3 WHERE id=$4")
        .bind(status).bind(comment)
        .bind(timestamp(now.to_timezone(time_tz::timezones::db::europe::PRAGUE)))
        .bind(id).execute(&mut *tx).await?;
    if !remove_ids.is_empty() {
        sqlx::query("DELETE FROM issue_report_resolution_photos WHERE report_id=$1 AND id=ANY($2)")
            .bind(id)
            .bind(remove_ids.into_iter().collect::<Vec<_>>())
            .execute(&mut *tx)
            .await?;
    }
    for photo in photos {
        sqlx::query("INSERT INTO issue_report_resolution_photos(report_id,photo) VALUES ($1,$2)")
            .bind(id)
            .bind(photo)
            .execute(&mut *tx)
            .await?;
    }
    audit(&mut tx, Some(admin.id), "updated", "issue_report", id, now).await?;
    let report = sqlx::query_as(ADMIN_REPORT_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(report))
}

pub async fn settings(State(s): State<Backend>, _: auth::Admin) -> Result<Json<ReportSettings>> {
    Ok(Json(ReportSettings {
        recipients: sqlx::query_scalar("SELECT recipients FROM report_settings WHERE id=1")
            .fetch_one(&s.pool)
            .await?,
    }))
}

pub async fn update_settings(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<ReportSettings>,
) -> Result<Json<ReportSettings>> {
    if input.recipients.len() > RECIPIENT_LIMIT {
        return Err(bad("Lze nastavit nejvýše 20 e-mailových adres."));
    }
    let recipients: Vec<String> = input
        .recipients
        .iter()
        .map(|value| auth::email(value))
        .collect::<Result<BTreeSet<_>>>()?
        .into_iter()
        .collect();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    sqlx::query("UPDATE report_settings SET recipients=$1 WHERE id=1")
        .bind(&recipients)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "report_settings",
        1,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ReportSettings { recipients }))
}

async fn photo_response(s: &Backend, id: i64, position: i64, admin: bool) -> Result<Response> {
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT p.photo FROM issue_report_photos p JOIN issue_reports r ON r.id=p.report_id WHERE r.id=$1 AND p.position=$2 AND (r.hidden=FALSE OR $3)")
        .bind(id).bind(position).bind(admin).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    Ok(image_response(bytes))
}

fn image_response(bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"zavada.jpg\"",
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'",
            ),
        ],
        Body::from(bytes),
    )
        .into_response()
}

pub async fn photo(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Response> {
    photo_response(&s, id, 1, false).await
}

pub async fn admin_photo(
    State(s): State<Backend>,
    _: auth::Admin,
    Path(id): Path<i64>,
) -> Result<Response> {
    photo_response(&s, id, 1, true).await
}

pub async fn gallery_photo(
    State(s): State<Backend>,
    Path((id, position)): Path<(i64, i64)>,
) -> Result<Response> {
    photo_response(&s, id, position, false).await
}

pub async fn admin_gallery_photo(
    State(s): State<Backend>,
    _: auth::Admin,
    Path((id, position)): Path<(i64, i64)>,
) -> Result<Response> {
    photo_response(&s, id, position, true).await
}

async fn resolution_photo_response(
    s: &Backend,
    id: i64,
    photo_id: i64,
    admin: bool,
) -> Result<Response> {
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT p.photo FROM issue_report_resolution_photos p JOIN issue_reports r ON r.id=p.report_id WHERE r.id=$1 AND p.id=$2 AND (r.hidden=FALSE OR $3)")
        .bind(id).bind(photo_id).bind(admin).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    Ok(image_response(bytes))
}

pub async fn resolution_photo(
    State(s): State<Backend>,
    Path((id, photo_id)): Path<(i64, i64)>,
) -> Result<Response> {
    resolution_photo_response(&s, id, photo_id, false).await
}

pub async fn admin_resolution_photo(
    State(s): State<Backend>,
    _: auth::Admin,
    Path((id, photo_id)): Path<(i64, i64)>,
) -> Result<Response> {
    resolution_photo_response(&s, id, photo_id, true).await
}

#[derive(sqlx::FromRow)]
struct Pending {
    id: i64,
    recipient: String,
    subject: String,
    body: Option<String>,
    attempts: i64,
}

/// SMTP delivery is at least once. Stable Message-ID helps with deduplication
/// after a crash between SMTP acceptance and marking the message delivered.
pub async fn deliver_one(
    s: &Backend,
    smtp: &AsyncSmtpTransport<Tokio1Executor>,
    now: OffsetDateTime,
) -> anyhow::Result<bool> {
    let lease = auth::token();
    let message: Option<Pending> = sqlx::query_as("UPDATE report_mail_queue SET lock_token=$1,locked_until=$2,attempts=attempts+1 WHERE id=(SELECT id FROM report_mail_queue WHERE sent_at IS NULL AND next_attempt_at<=$3 AND locked_until<=$3 ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,recipient,subject,body,attempts")
        .bind(&lease).bind(now.unix_timestamp() + 300).bind(now.unix_timestamp()).fetch_optional(&s.pool).await?;
    let Some(message) = message else {
        return Ok(false);
    };
    let outcome = async {
        let email = Message::builder()
            .from(s.config.email_from.parse()?)
            .to(message.recipient.parse()?)
            .message_id(Some(format!(
                "<vysker-report-{}@{}>",
                message.id,
                url::Url::parse(&s.config.public_url)?
                    .host_str()
                    .unwrap_or("localhost")
            )))
            .subject(message.subject)
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(
                message
                    .body
                    .ok_or_else(|| anyhow::anyhow!("Missing report notification body"))?,
            )?;
        smtp.send(email).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if outcome.is_ok() {
        sqlx::query("UPDATE report_mail_queue SET sent_at=$1,body=NULL,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3")
            .bind(now.unix_timestamp()).bind(message.id).bind(&lease).execute(&s.pool).await?;
    } else {
        let delay = (30_i64 * 2_i64.pow(message.attempts.min(10) as u32)).min(21600);
        sqlx::query("UPDATE report_mail_queue SET next_attempt_at=$1,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3")
            .bind(now.unix_timestamp() + delay).bind(message.id).bind(&lease).execute(&s.pool).await?;
        tracing::warn!(
            message_id = message.id,
            "Report SMTP failed, retry scheduled"
        );
    }
    Ok(true)
}
