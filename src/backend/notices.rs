use super::{
    Backend, Pagination, Result, audit, auth::Admin, bad, conflict, documents::Attachment, missing,
    text, timestamp, today,
};
use crate::notice_policy::NoticeReview;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoticeInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub reference_number: Option<String>,
    pub category_id: Option<i64>,
    pub issuer: Option<String>,
    pub published_on: Option<Date>,
    pub withdraw_on: Option<Date>,
    pub duration_days: Option<i64>,
    #[serde(default)]
    pub unlimited: bool,
    pub retain_attachments: Option<bool>,
    #[serde(default)]
    pub review: NoticeReview,
}
impl NoticeInput {
    pub fn dates(&self, now: OffsetDateTime) -> Result<(Date, Option<Date>)> {
        if self.unlimited {
            return Err(bad("Vyplňte počet dní vyvěšení."));
        }
        let posted = self.published_on.unwrap_or_else(|| today(now));
        Ok((
            posted,
            Some(super::registry::posting_end(
                posted,
                self.duration_days,
                self.withdraw_on,
            )?),
        ))
    }
    async fn validate(&self, conn: &mut PgConnection) -> Result<()> {
        text(&self.title, 300)?;
        if self.retain_attachments.is_none() {
            return Err(bad("Vyberte dostupnost příloh po sejmutí."));
        }
        if self.description.len() > 50_000
            || self
                .reference_number
                .as_ref()
                .is_some_and(|s| s.len() > 200)
            || self.issuer.as_ref().is_some_and(|s| s.len() > 300)
        {
            return Err(bad("Text dokumentu je příliš dlouhý."));
        }
        if let Some(id) = self.category_id {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE id=$1)")
                    .bind(id)
                    .fetch_one(conn)
                    .await?;
            if !exists {
                return Err(bad("Téma neexistuje."));
            }
        }
        Ok(())
    }
}
#[derive(Clone, sqlx::FromRow, Serialize)]
pub struct NoticeRecord {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub reference_number: Option<String>,
    pub category_id: Option<i64>,
    pub issuer: Option<String>,
    pub published_on: Option<Date>,
    pub withdraw_on: Option<Date>,
    pub withdrawn_at: Option<String>,
    pub retain_attachments: bool,
    pub status: String,
    pub review_json: String,
    pub published_at: Option<String>,
    pub lock_state: String,
    pub public_access: bool,
    pub access_blocked: bool,
    pub public_until: Option<Date>,
    pub content_version: i64,
    pub metadata_restricted: bool,
    pub public_title: Option<String>,
    pub attachments_public: bool,
}
#[derive(Serialize)]
pub struct NoticeDetail {
    #[serde(flatten)]
    pub record: NoticeRecord,
    pub attachments: Vec<Attachment>,
    pub postings: Vec<serde_json::Value>,
    pub relations: Vec<serde_json::Value>,
    pub origin: Option<serde_json::Value>,
}

pub async fn create(
    State(s): State<Backend>,
    admin: Admin,
    Json(input): Json<NoticeInput>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let now = OffsetDateTime::now_utc();
    let (posted, end) = input.dates(now)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    input.validate(&mut tx).await?;

    let id = sqlx::query_scalar::<_, i64>("INSERT INTO notices(title,description,reference_number,category_id,issuer,published_on,withdraw_on,retain_attachments,status,created_at,updated_at,review_json) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'draft',$9,$10,$11) RETURNING id")
        .bind(input.title.trim()).bind(input.description).bind(input.reference_number).bind(input.category_id).bind(input.issuer)
        .bind(posted).bind(end).bind(input.retain_attachments.unwrap_or(false)).bind(timestamp(now)).bind(timestamp(now)).bind(serde_json::to_string(&input.review).unwrap()).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE documents SET posting_after_action=CASE WHEN retain_attachments THEN 'retain' ELSE 'hide' END WHERE id=$1").bind(id).execute(&mut *tx).await?;
    audit(&mut tx, Some(admin.id), "created", "notice", id, now).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id":id}))))
}
pub async fn update(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<NoticeInput>,
) -> Result<StatusCode> {
    let now = OffsetDateTime::now_utc();
    let (posted, end) = input.dates(now)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    input.validate(&mut tx).await?;

    let old: Option<NoticeRecord> = sqlx::query_as("SELECT * FROM notices WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let old = old.ok_or_else(missing)?;
    if old.status != "draft" || old.lock_state != "draft" {
        return Err(conflict(
            "Upravovat lze pouze koncept. Zveřejněné znění musí zůstat doložitelné.",
        ));
    }
    if old.status == "published"
        && (Some(posted) != old.published_on || end.is_some_and(|d| d <= today(now)))
    {
        return Err(conflict(
            "U vyvěšeného záznamu nelze změnit den vyvěšení ani nastavit uplynulou lhůtu. Použijte sejmutí.",
        ));
    }
    sqlx::query("UPDATE notices SET title=$1,description=$2,reference_number=$3,category_id=$4,issuer=$5,published_on=$6,withdraw_on=$7,retain_attachments=$8,updated_at=$9,review_json=$10 WHERE id=$11")
        .bind(input.title.trim()).bind(input.description).bind(input.reference_number).bind(input.category_id).bind(input.issuer)
        .bind(posted).bind(end).bind(input.retain_attachments.unwrap_or(false)).bind(timestamp(now)).bind(serde_json::to_string(&input.review).unwrap()).bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE documents SET posting_after_action=CASE WHEN retain_attachments THEN 'retain' ELSE 'hide' END WHERE id=$1").bind(id).execute(&mut *tx).await?;
    audit(&mut tx, Some(admin.id), "updated", "notice", id, now).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishInput {
    pub token: String,
}
pub async fn publish(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<PublishInput>,
) -> Result<StatusCode> {
    publish_checked(
        &s,
        id,
        admin.id,
        OffsetDateTime::now_utc(),
        Some(input.token),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn publish_at(s: &Backend, id: i64, actor: i64, now: OffsetDateTime) -> Result<()> {
    publish_checked(s, id, actor, now, None).await
}
async fn publish_checked(
    s: &Backend,
    id: i64,
    actor: i64,
    now: OffsetDateTime,
    token: Option<String>,
) -> Result<()> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let record: NoticeRecord = sqlx::query_as("SELECT * FROM notices WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    if record.status != "draft" || record.lock_state != "draft" {
        return Err(conflict("Zveřejnit lze pouze koncept."));
    }
    let current_hash = super::registry::fingerprint(&mut tx, id).await?;
    if token.as_ref().is_some_and(|t| t != &current_hash) {
        return Err(conflict("Obsah se změnil. Načtěte nový finální náhled."));
    }
    sqlx::query("UPDATE documents SET posting_after_action=CASE WHEN retain_attachments THEN 'retain' ELSE 'hide' END,posting_public_until=NULL,scheduled_sha256=$2 WHERE id=$1").bind(id).bind(current_hash).execute(&mut *tx).await?;
    let posted = record
        .published_on
        .ok_or_else(|| bad("Datum vyvěšení není uvedeno."))?;
    if posted < today(now) {
        return Err(bad(
            "Zveřejnění nelze zpětně datovat. Upravte datum konceptu.",
        ));
    }
    if record.withdraw_on.is_none() {
        return Err(bad("Vyplňte počet dní vyvěšení."));
    }
    if record.withdraw_on.is_some_and(|d| d <= today(now)) {
        return Err(bad("Lhůta dokumentu už uplynula."));
    }
    let status = if posted > today(now) {
        "scheduled"
    } else {
        "published"
    };
    sqlx::query("UPDATE notices SET status=$1,updated_at=$2 WHERE id=$3")
        .bind(status)
        .bind(timestamp(now))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, Some(actor), status, "notice", id, now).await?;
    if status == "published" {
        sqlx::query("UPDATE notices SET published_at=$1 WHERE id=$2")
            .bind(timestamp(now))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        evidence(
            &mut tx,
            id,
            Some(actor),
            "published",
            serde_json::json!({}),
            now,
        )
        .await?;
        super::registry::start_posting(&mut tx, id, Some(actor), "Schválený náhled", now).await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn withdraw(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    input: Option<Json<WithdrawalInput>>,
) -> Result<StatusCode> {
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let record: NoticeRecord = sqlx::query_as("SELECT * FROM notices WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(missing)?;
    let input = input.map(|v| v.0).unwrap_or_default();
    if input
        .expected_status
        .as_deref()
        .is_some_and(|expected| expected != record.status)
    {
        return Err(conflict(
            "Stav dokumentu se mezitím změnil. Načtěte jej znovu a ověřte požadovanou změnu.",
        ));
    }
    if record.status == "scheduled" {
        if input.emergency || s.config.production {
            text(&input.reason, 2000)?;
        }
        return_to_draft(
            &mut tx,
            id,
            Some(admin.id),
            "schedule_cancelled",
            serde_json::json!({"reason":input.reason}),
            now,
        )
        .await?;
        tx.commit().await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    if input.emergency || s.config.production {
        text(&input.reason, 2000)?;
    }
    evidence(
        &mut tx,
        id,
        Some(admin.id),
        if input.emergency {
            "emergency_withdrawal"
        } else {
            "manual_withdrawal"
        },
        serde_json::json!({"reason":input.reason}),
        now,
    )
    .await?;
    archive_record(&mut tx, id, Some(admin.id), now).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn return_to_draft(
    conn: &mut PgConnection,
    id: i64,
    actor: Option<i64>,
    operation: &str,
    details: serde_json::Value,
    now: OffsetDateTime,
) -> Result<()> {
    // A cancelled or missed schedule never made the record public. Keep its
    // attachments available to editors and do not create a public archive entry.
    sqlx::query("UPDATE documents SET board_status=CASE WHEN has_removed_posting THEN 'archived' ELSE 'draft' END,updated_at=$1 WHERE id=$2")
        .bind(timestamp(now))
        .bind(id)
        .execute(&mut *conn)
        .await?;
    evidence(conn, id, actor, operation, details, now).await?;
    audit(conn, actor, operation, "notice", id, now).await
}

async fn archive_record(
    conn: &mut PgConnection,
    id: i64,
    actor: Option<i64>,
    now: OffsetDateTime,
) -> Result<()> {
    let postings:Vec<i64>=sqlx::query_scalar("SELECT id FROM document_postings WHERE document_id=$1 AND certainty='documented' AND removed_on IS NULL AND removed_at IS NULL ORDER BY id").bind(id).fetch_all(&mut *conn).await?;
    if postings.is_empty() {
        return Err(conflict("Není doloženo probíhající vyvěšení."));
    }
    for posting in postings {
        super::registry::remove_posting(conn, posting, actor, "Sejmutí", now).await?;
    }

    evidence(conn, id, actor, "withdrawn", serde_json::json!({}), now).await?;
    audit(conn, actor, "withdrawn", "notice", id, now).await
}

/// Dohání i termíny z doby, kdy server neběžel. Opakovaný běh je idempotentní.
pub async fn maintenance(s: &Backend, now: OffsetDateTime) -> Result<()> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let expired: Vec<i64> = sqlx::query_scalar("SELECT d.id FROM documents d WHERE d.board_status='published' AND d.withdraw_on<=$1 AND EXISTS(SELECT 1 FROM document_postings p WHERE p.document_id=d.id AND p.origin='local' AND p.removed_at IS NULL AND p.removed_on IS NULL) ORDER BY d.id LIMIT 500")
        .bind(today(now)).fetch_all(&mut *tx).await?;
    for id in expired {
        archive_record(&mut tx, id, None, now).await?;
    }
    let scheduled: Vec<NoticeRecord> = sqlx::query_as(
        "SELECT * FROM notices WHERE status='scheduled' AND published_on<=$1 ORDER BY id LIMIT 500",
    )
    .bind(today(now))
    .fetch_all(&mut *tx)
    .await?;
    for record in scheduled {
        if record.access_blocked {
            return_to_draft(
                &mut tx,
                record.id,
                None,
                "schedule_cancelled",
                serde_json::json!({"reason":"Místní zákaz zveřejnění"}),
                now,
            )
            .await?;
            continue;
        }
        if record
            .published_on
            .is_some_and(|posted| posted < today(now))
        {
            return_to_draft(
                &mut tx,
                record.id,
                None,
                "schedule_missed",
                serde_json::json!({}),
                now,
            )
            .await?;
            continue;
        }
        sqlx::query(
            "UPDATE notices SET status='published',updated_at=$1,published_at=$2 WHERE id=$3",
        )
        .bind(timestamp(now))
        .bind(timestamp(now))
        .bind(record.id)
        .execute(&mut *tx)
        .await?;
        audit(&mut tx, None, "published", "notice", record.id, now).await?;
        evidence(
            &mut tx,
            record.id,
            None,
            "published",
            serde_json::json!({}),
            now,
        )
        .await?;
        super::registry::start_posting(
            &mut tx,
            record.id,
            None,
            "Plánované vyvěšení schválené podoby",
            now,
        )
        .await?;
    }
    sqlx::query("UPDATE documents SET attachments_public=FALSE WHERE board_status IN ('archived','withdrawn') AND attachments_public AND posting_public_until<=$1").bind(today(now)).execute(&mut *tx).await?;
    // Úklid relací a tokenů.
    sqlx::query("DELETE FROM sessions WHERE expires_at<=$1")
        .bind(now.unix_timestamp())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM subscription_tokens WHERE expires_at<=$1")
        .bind(now.unix_timestamp())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM rate_limits WHERE window_started_at<$1")
        .bind(now.unix_timestamp() - 86400)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn admin_list(
    State(s): State<Backend>,
    _: Admin,
    Query(p): Query<Pagination>,
) -> Result<Json<Vec<NoticeRecord>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(
        sqlx::query_as("SELECT * FROM notices ORDER BY published_on DESC NULLS LAST,id DESC LIMIT $1 OFFSET $2")
            .bind(limit)
            .bind(offset)
            .fetch_all(&s.pool)
            .await?,
    ))
}
#[derive(Deserialize, Default)]
pub struct NoticeQuery {
    #[serde(default)]
    pub archived: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
pub async fn public_list(
    State(s): State<Backend>,
    Query(q): Query<NoticeQuery>,
) -> Result<Json<Vec<NoticeRecord>>> {
    let (limit, offset) = Pagination {
        limit: q.limit,
        offset: q.offset,
    }
    .bounds()?;
    let now = OffsetDateTime::now_utc();
    let rows:Vec<NoticeRecord>=sqlx::query_as("SELECT n.* FROM notices n JOIN documents d ON d.id=n.id WHERE document_is_public(d,$2) AND (($1=FALSE AND n.status='published' AND d.has_current_posting) OR ($1=TRUE AND n.status IN ('archived','withdrawn') AND d.has_removed_posting)) ORDER BY n.published_on DESC NULLS LAST,n.id DESC LIMIT $3 OFFSET $4")
        .bind(q.archived).bind(today(now)).bind(limit).bind(offset).fetch_all(&s.pool).await?;
    Ok(Json(rows.into_iter().map(|r| r.into_public(now)).collect()))
}
pub async fn detail(State(s): State<Backend>, Path(id): Path<i64>) -> Result<Json<NoticeDetail>> {
    let record:NoticeRecord = sqlx::query_as(
        "SELECT n.* FROM notices n JOIN documents d ON d.id=n.id WHERE n.id=$1 AND document_is_public(d,$2)",
    ).bind(id).bind(today(OffsetDateTime::now_utc())).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    let files =
        super::documents::public_notice_files(&s.pool, id, OffsetDateTime::now_utc()).await?;
    Ok(Json(NoticeDetail {
        record: record.into_public(OffsetDateTime::now_utc()),
        attachments: files,
        postings: super::registry::postings(&s.pool, id, true).await?,
        relations: super::documents::public_relations(&s.pool, id).await?,
        origin: None,
    }))
}

#[derive(sqlx::FromRow, Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
}
pub async fn categories(State(s): State<Backend>) -> Result<Json<Vec<Category>>> {
    Ok(Json(
        sqlx::query_as("SELECT id,name FROM categories ORDER BY sort_order,id")
            .fetch_all(&s.pool)
            .await?,
    ))
}

impl NoticeRecord {
    pub fn review(&self) -> Result<NoticeReview> {
        serde_json::from_str(&self.review_json)
            .map_err(|_| conflict("Poškozené nastavení vyvěšení."))
    }
    pub fn active(&self, _now: OffsetDateTime) -> bool {
        self.status == "published" && self.public_access && !self.access_blocked
    }
    pub fn files_public(&self, now: OffsetDateTime) -> bool {
        self.public_access
            && !self.access_blocked
            && self.public_until.is_none_or(|d| d > today(now))
            && (self.status == "published" || self.retain_attachments)
    }
    pub fn into_public(mut self, _now: OffsetDateTime) -> Self {
        if self.metadata_restricted {
            self.title = self
                .public_title
                .clone()
                .unwrap_or_else(|| format!("Záznam úřední desky #{}", self.id));
            self.description = None;
            self.reference_number = None;
            self.issuer = None;
        }
        self.review_json = "{}".into();
        self
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalInput {
    pub expected_status: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub emergency: bool,
}

async fn evidence(
    conn: &mut PgConnection,
    id: i64,
    actor: Option<i64>,
    kind: &str,
    extra: serde_json::Value,
    now: OffsetDateTime,
) -> Result<()> {
    let record: NoticeRecord = sqlx::query_as("SELECT * FROM notices WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let files: Vec<(i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT id,sha256,size_bytes FROM attachments WHERE notice_id=$1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    // Only fingerprints, identifiers and dates. The originals belong to the municipality's records system.
    let payload=serde_json::json!({"notice_id":id,"record_sha256":super::auth::hash(serde_json::to_vec(&record).unwrap()),"published_on":record.published_on,"published_at":record.published_at,"withdraw_on":record.withdraw_on,"withdrawn_at":record.withdrawn_at,"files":files,"details":extra}).to_string();
    sqlx::query("INSERT INTO notice_events(notice_id,occurred_at,actor_id,kind,payload,sha256) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(timestamp(now)).bind(actor).bind(kind).bind(&payload).bind(super::auth::hash(&payload)).execute(&mut *conn).await?;
    Ok(())
}

#[derive(sqlx::FromRow, Serialize)]
pub struct NoticeEvent {
    id: i64,
    occurred_at: String,
    actor_id: Option<i64>,
    kind: String,
    payload: String,
    sha256: String,
}
pub async fn publication_evidence(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>> {
    let record: NoticeRecord = sqlx::query_as("SELECT * FROM notices WHERE id=$1")
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(missing)?;
    let events: Vec<NoticeEvent> =
        sqlx::query_as("SELECT * FROM notice_events WHERE notice_id=$1 ORDER BY id")
            .bind(id)
            .fetch_all(&s.pool)
            .await?;
    Ok(Json(
        serde_json::json!({"notice":record,"events":events,"availability_note":"Události dokládají změny aplikace. Nepotvrzují nepřetržitou dostupnost webu ani vyvěšení na fyzické desce."}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentInput {
    pub started_at: String,
    pub ended_at: String,
    pub reason: String,
}
pub async fn incident(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<IncidentInput>,
) -> Result<StatusCode> {
    let now = OffsetDateTime::now_utc();
    text(&input.reason, 2000)?;
    let started_at = OffsetDateTime::parse(
        &input.started_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| bad("Neplatný začátek výpadku."))?;
    let ended_at = OffsetDateTime::parse(
        &input.ended_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| bad("Neplatný konec výpadku."))?;
    if ended_at < started_at || ended_at > now {
        return Err(bad("Neplatné období výpadku."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM notices WHERE id=$1 AND published_at IS NOT NULL)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if !exists {
        return Err(conflict(
            "Výpadek lze zaznamenat ke skutečně zveřejněnému záznamu.",
        ));
    }
    evidence(&mut tx,id,Some(admin.id),"availability_incident",serde_json::json!({"started_at":input.started_at,"ended_at":input.ended_at,"reason":input.reason}),now).await?;
    audit(
        &mut tx,
        Some(admin.id),
        "availability_incident",
        "notice",
        id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}
