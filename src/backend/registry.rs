//! Canonical document operations. Every write shares the import/editor transaction lock.
use super::{Backend, Result, auth::Admin, bad, conflict, missing, text, timestamp, today};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use time::{Date, OffsetDateTime};

pub async fn event(
    conn: &mut PgConnection,
    id: i64,
    actor: Option<i64>,
    kind: &str,
    reason: &str,
    payload: Value,
) -> Result<()> {
    sqlx::query("INSERT INTO document_events(document_id,actor_id,kind,reason,payload) VALUES($1,$2,$3,$4,$5)").bind(id).bind(actor).bind(kind).bind(reason).bind(payload).execute(conn).await?;
    Ok(())
}
pub async fn fingerprint(conn: &mut PgConnection, id: i64) -> Result<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT document_fingerprint($1)")
        .bind(id)
        .fetch_one(conn)
        .await?
        .ok_or_else(missing)
}
pub async fn preview(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let document: super::documents::Document =
        sqlx::query_as("SELECT * FROM documents WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(missing)?;
    let attachments:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'content_type',content_type,'size_bytes',size_bytes,'sha256',sha256,'available',data IS NOT NULL AND removed_at IS NULL) FROM attachments WHERE document_id=$1 ORDER BY sort_order,id").bind(id).fetch_all(&mut *tx).await?;
    let token = fingerprint(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"token":token,"document":document,"attachments":attachments}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostingInput {
    pub token: String,
    pub published_on: Date,
    pub withdraw_on: Option<Date>,
    pub after_action: String,
    pub public_until: Option<Date>,
    pub redacted_document_id: Option<i64>,
    pub redacted_token: Option<String>,
    pub reason: String,
    pub review: Option<crate::notice_policy::NoticeReview>,
}
pub async fn post(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<PostingInput>,
) -> Result<StatusCode> {
    let now = OffsetDateTime::now_utc();
    text(&input.reason, 2000)?;
    if !["retain", "hide", "anonymized"].contains(&input.after_action.as_str()) {
        return Err(bad(
            "Vyberte, zda budou přílohy po sejmutí veřejné, nebo dostupné pouze v administraci. Volba nemá výchozí hodnotu.",
        ));
    }
    if input.published_on < today(now) || input.withdraw_on.is_some_and(|d| d <= input.published_on)
    {
        return Err(bad("Neplatné datum vyvěšení nebo plánovaného sejmutí."));
    }
    if input
        .public_until
        .is_some_and(|d| d <= input.withdraw_on.unwrap_or(input.published_on))
    {
        return Err(bad(
            "Konec veřejného zpřístupnění musí následovat po sejmutí.",
        ));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let (state,board,blocked,review):(String,Option<String>,bool,String)=sqlx::query_as("SELECT lock_state,board_status,access_blocked,review_json FROM documents WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(missing)?;
    if state == "review" || blocked || matches!(board.as_deref(), Some("published" | "scheduled")) {
        return Err(conflict(
            "Dokument vyžaduje ověření, je místně stažený nebo už má probíhající vyvěšení.",
        ));
    }
    if fingerprint(&mut tx, id).await? != input.token {
        return Err(conflict(
            "Obsah nebo přílohy se změnily. Načtěte nový finální náhled.",
        ));
    }
    let incomplete:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attachments WHERE document_id=$1 AND removed_at IS NULL AND data IS NULL)").bind(id).fetch_one(&mut *tx).await?;
    if incomplete {
        return Err(conflict(
            "Některá příloha není úplně převzatá. Před vyvěšením doplňte chybějící soubory.",
        ));
    }
    let review: crate::notice_policy::NoticeReview = input
        .review
        .unwrap_or(serde_json::from_str(&review).map_err(|_| bad("Neplatné údaje kontroly."))?);
    let earliest = review.earliest(input.published_on).map_err(bad)?;
    if input
        .withdraw_on
        .is_some_and(|d| earliest.is_some_and(|e| d < e))
    {
        return Err(bad(
            "Plán je před nejdřívějším termínem zvoleného pravidla.",
        ));
    }
    if s.config.production
        && (!review.reviewed
            || review.rule.is_empty()
            || review.legal_basis.trim().is_empty()
            || review.original_reference.trim().is_empty())
    {
        return Err(bad(
            "Před vyvěšením vyplňte kontrolu obsahu, právní podklad a evidenci originálu.",
        ));
    }
    sqlx::query("UPDATE documents SET review_json=$2 WHERE id=$1")
        .bind(id)
        .bind(serde_json::to_string(&review).unwrap())
        .execute(&mut *tx)
        .await?;
    event(
        &mut tx,
        id,
        Some(admin.id),
        "posting_review",
        &input.reason,
        json!({"review":review}),
    )
    .await?;
    if input.after_action == "anonymized" {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_relations r JOIN documents d ON d.id=r.related_id WHERE original_id=$1 AND related_id=$2 AND kind='anonymized' AND NOT d.access_blocked)").bind(id).bind(input.redacted_document_id).fetch_one(&mut *tx).await?;
        if !valid {
            return Err(bad("Vyberte samostatné propojené anonymizované znění."));
        }
        let redacted = input
            .redacted_document_id
            .ok_or_else(|| bad("Chybí anonymizované znění."))?;
        let (state, disposed): (String, bool) = sqlx::query_as(
            "SELECT lock_state,disposed_at IS NOT NULL FROM documents WHERE id=$1 FOR UPDATE",
        )
        .bind(redacted)
        .fetch_one(&mut *tx)
        .await?;
        if state == "review" || disposed {
            return Err(conflict(
                "Anonymizované znění vyžaduje kontrolu nebo bylo skartováno.",
            ));
        }
        let redacted_hash = fingerprint(&mut tx, redacted).await?;
        if input.redacted_token.as_deref() != Some(redacted_hash.as_str()) {
            return Err(conflict(
                "Načtěte a potvrďte aktuální náhled anonymizovaného znění.",
            ));
        }
        let incomplete:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attachments WHERE document_id=$1 AND removed_at IS NULL AND data IS NULL)").bind(redacted).fetch_one(&mut *tx).await?;
        if incomplete {
            return Err(conflict("Anonymizované znění má chybějící přílohu."));
        }
        if state == "draft" {
            sqlx::query("UPDATE documents SET lock_state='locked' WHERE id=$1")
                .bind(redacted)
                .execute(&mut *tx)
                .await?;
            event(
                &mut tx,
                redacted,
                Some(admin.id),
                "anonymized_snapshot_frozen",
                &input.reason,
                json!({"original_id":id,"sha256":redacted_hash,"posting_claim":false}),
            )
            .await?;
        }
    }
    sqlx::query("UPDATE documents SET published_on=$2,withdraw_on=$3,retain_attachments=$4,posting_after_action=$5,posting_public_until=$6,posting_redacted_id=$7,scheduled_sha256=$8,board_status='draft' WHERE id=$1")
      .bind(id).bind(input.published_on).bind(input.withdraw_on).bind(input.after_action=="retain").bind(&input.after_action).bind(input.public_until).bind(input.redacted_document_id).bind(&input.token).execute(&mut *tx).await?;
    if input.published_on > today(now) {
        sqlx::query("UPDATE documents SET board_status='scheduled' WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        event(&mut tx,id,Some(admin.id),"posting_scheduled",&input.reason,json!({"planned_on":input.published_on,"sha256":input.token,"after_action":input.after_action})).await?;
    } else {
        start_posting(&mut tx, id, Some(admin.id), &input.reason, now).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}
/// Called only after final review, or by the scheduled worker with the approved hash.
pub async fn start_posting(
    conn: &mut PgConnection,
    id: i64,
    actor: Option<i64>,
    reason: &str,
    now: OffsetDateTime,
) -> Result<()> {
    let (locked,approved,after_action,public_until,redacted):(String,Option<String>,Option<String>,Option<Date>,Option<i64>)=sqlx::query_as("SELECT lock_state,scheduled_sha256,posting_after_action,posting_public_until,posting_redacted_id FROM documents WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *conn).await?;
    let blocked: bool = sqlx::query_scalar(
        "SELECT access_blocked OR disposed_at IS NOT NULL FROM documents WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if blocked {
        return Err(conflict(
            "Místní zákaz zveřejnění nebo skartace brání vyvěšení.",
        ));
    }
    let hash = fingerprint(conn, id).await?;
    if approved.as_ref().is_some_and(|v| v != &hash) {
        return Err(conflict(
            "Plánovaný dokument se změnil. Je nutný nový náhled.",
        ));
    }
    let action = after_action
        .ok_or_else(|| bad("Chybí povinné rozhodnutí o dostupnosti příloh po sejmutí."))?;
    if locked == "review" {
        return Err(conflict("Historie dokumentu vyžaduje ověření."));
    }
    sqlx::query("UPDATE documents SET lock_state='locked',public_access=TRUE,status='published',attachments_public=TRUE,metadata_restricted=FALSE,public_until=NULL,board_status='published',board_published_at=$2,published_at=coalesce(published_at,$2),withdrawn_at=NULL WHERE id=$1 AND NOT access_blocked").bind(id).bind(timestamp(now)).execute(&mut *conn).await?;
    let posting_id:i64=sqlx::query_scalar("INSERT INTO document_postings(document_id,origin,certainty,started_on,started_at,planned_removal_on,after_action,public_until,redacted_document_id,snapshot_sha256) SELECT id,'local','documented',published_on,$2,withdraw_on,$3,$4,$5,$6 FROM documents WHERE id=$1 RETURNING id").bind(id).bind(now).bind(&action).bind(public_until).bind(redacted).bind(hash).fetch_one(&mut *conn).await?;
    event(
        conn,
        id,
        actor,
        "posted",
        reason,
        json!({"posting_id":posting_id,"after_action":action}),
    )
    .await?;
    super::mail::enqueue_publication(conn, super::mail::PublicationTarget::Notice(id), now).await?;
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reason {
    pub reason: String,
    #[serde(default)]
    pub emergency: bool,
}
pub async fn withdraw(
    State(s): State<Backend>,
    admin: Admin,
    Path(posting): Path<i64>,
    Json(input): Json<Reason>,
) -> Result<StatusCode> {
    text(&input.reason, 2000)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    if posting_earliest(&mut tx, posting)
        .await?
        .is_some_and(|day| today(OffsetDateTime::now_utc()) < day)
        && !input.emergency
    {
        return Err(conflict(
            "Minimální termín zvoleného pravidla dosud neuplynul. Předčasné sejmutí vyžaduje výslovně označený incident s důvodem. Veřejný přístup lze okamžitě ukončit samostatným úkonem.",
        ));
    }
    if input.emergency {
        let id: i64 = sqlx::query_scalar("SELECT document_id FROM document_postings WHERE id=$1")
            .bind(posting)
            .fetch_one(&mut *tx)
            .await?;
        event(
            &mut tx,
            id,
            Some(admin.id),
            "emergency_removal_requested",
            &input.reason,
            json!({"posting_id":posting}),
        )
        .await?;
    }
    remove_posting(
        &mut tx,
        posting,
        Some(admin.id),
        &input.reason,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn remove_posting(
    conn: &mut PgConnection,
    posting: i64,
    actor: Option<i64>,
    reason: &str,
    now: OffsetDateTime,
) -> Result<()> {
    let (id,action,until,redacted):(i64,String,Option<Date>,Option<i64>)=sqlx::query_as("SELECT document_id,after_action,public_until,redacted_document_id FROM document_postings WHERE id=$1 AND certainty='documented' AND removed_at IS NULL AND removed_on IS NULL FOR UPDATE").bind(posting).fetch_optional(&mut *conn).await?.ok_or_else(||conflict("Vyvěšení již bylo ukončeno nebo není doloženo."))?;
    sqlx::query("UPDATE document_postings SET removed_at=$2,removed_on=$3 WHERE id=$1")
        .bind(posting)
        .bind(now)
        .bind(today(now))
        .execute(&mut *conn)
        .await?;
    let remaining:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_postings WHERE document_id=$1 AND certainty='documented' AND removed_at IS NULL AND removed_on IS NULL)").bind(id).fetch_one(&mut *conn).await?;
    if !remaining {
        // Privacy is an access decision. Neither locked bytes nor filenames are changed.
        sqlx::query("UPDATE documents SET board_status='archived',withdrawn_at=$2,attachments_public=$3,public_until=$4 WHERE id=$1").bind(id).bind(timestamp(now)).bind(action=="retain").bind(until).execute(&mut *conn).await?;
        if action == "anonymized" {
            sqlx::query("UPDATE documents SET public_access=FALSE,access_blocked=TRUE,status='archived' WHERE id=$1")
                .bind(id)
                .execute(&mut *conn)
                .await?;
            let released=sqlx::query("UPDATE documents d SET public_access=TRUE,status='published',published_at=coalesce(published_at,$2) WHERE id=$1 AND NOT access_blocked AND lock_state='locked' AND disposed_at IS NULL AND NOT EXISTS(SELECT 1 FROM attachments a WHERE a.document_id=d.id AND a.removed_at IS NULL AND a.data IS NULL)").bind(redacted).bind(timestamp(now)).execute(&mut *conn).await?.rows_affected();
            if released > 0 {
                let target = redacted.ok_or_else(missing)?;
                // Switching to an approved archive rendition is not a new bulletin.
                sqlx::query("SELECT claim_document_announcement($1,$2)")
                    .bind(target)
                    .bind(now)
                    .execute(&mut *conn)
                    .await?;
                event(
                    conn,
                    target,
                    actor,
                    "anonymized_archive_released",
                    reason,
                    json!({"original_id":id,"posting_claim":false}),
                )
                .await?;
            }
        }
    }
    event(
        conn,
        id,
        actor,
        "removed",
        reason,
        json!({"posting_id":posting,"actual_at":timestamp(now),"after_action":action}),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub withdraw_on: Option<Date>,
    pub reason: String,
}
pub async fn plan(
    State(s): State<Backend>,
    admin: Admin,
    Path(posting): Path<i64>,
    Json(input): Json<Plan>,
) -> Result<StatusCode> {
    text(&input.reason, 2000)?;
    if input
        .withdraw_on
        .is_some_and(|d| d <= today(OffsetDateTime::now_utc()))
    {
        return Err(bad(
            "Změna plánu se musí týkat budoucnosti. Pro skutečné sejmutí použijte samostatný úkon.",
        ));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let (id,old):(i64,Option<Date>)=sqlx::query_as("SELECT document_id,planned_removal_on FROM document_postings WHERE id=$1 AND removed_on IS NULL AND removed_at IS NULL FOR UPDATE").bind(posting).fetch_optional(&mut *tx).await?.ok_or_else(missing)?;
    let earliest = posting_earliest(&mut tx, posting).await?;
    if input
        .withdraw_on
        .is_some_and(|day| earliest.is_some_and(|minimum| day < minimum))
    {
        return Err(bad(
            "Plánované sejmutí je před minimálním termínem zvoleného pravidla.",
        ));
    }
    sqlx::query("UPDATE document_postings SET planned_removal_on=$2 WHERE id=$1")
        .bind(posting)
        .bind(input.withdraw_on)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE documents SET withdraw_on=$2 WHERE id=$1")
        .bind(id)
        .bind(input.withdraw_on)
        .execute(&mut *tx)
        .await?;
    event(
        &mut tx,
        id,
        Some(admin.id),
        "plan_changed",
        &input.reason,
        json!({"posting_id":posting,"old":old,"new":input.withdraw_on}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Visibility {
    pub public_access: bool,
    pub reason: String,
}
pub async fn visibility(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<Visibility>,
) -> Result<StatusCode> {
    text(&input.reason, 2000)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let (old, lock): (bool, String) =
        sqlx::query_as("SELECT public_access,lock_state FROM documents WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(missing)?;
    if input.public_access && lock == "review" {
        return Err(conflict(
            "Nejasnou historii nejprve ověřte. Tento stav nelze obejít zveřejněním.",
        ));
    }
    sqlx::query("UPDATE documents SET public_access=$2,access_blocked=NOT $2,status=CASE WHEN $2 THEN 'published' ELSE 'archived' END,published_at=CASE WHEN $2 THEN coalesce(published_at,$3) ELSE published_at END WHERE id=$1").bind(id).bind(input.public_access).bind(timestamp(OffsetDateTime::now_utc())).execute(&mut *tx).await?;
    event(
        &mut tx,
        id,
        Some(admin.id),
        "visibility_changed",
        &input.reason,
        json!({"old":old,"new":input.public_access}),
    )
    .await?;
    if input.public_access && !old {
        super::mail::enqueue_publication(
            &mut tx,
            super::mail::PublicationTarget::Document(id),
            OffsetDateTime::now_utc(),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topic {
    pub category_id: Option<i64>,
    pub reason: String,
}
pub async fn topic(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<Topic>,
) -> Result<StatusCode> {
    text(&input.reason, 2000)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    if let Some(category) = input.category_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE id=$1)")
                .bind(category)
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(bad("Téma neexistuje."));
        }
    }
    let old: Option<i64> =
        sqlx::query_scalar("SELECT category_id FROM documents WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(missing)?;
    sqlx::query("UPDATE documents SET category_id=$2 WHERE id=$1")
        .bind(id)
        .bind(input.category_id)
        .execute(&mut *tx)
        .await?;
    event(
        &mut tx,
        id,
        Some(admin.id),
        "catalogued",
        &input.reason,
        json!({"old":old,"new":input.category_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub kind: String,
    pub reason: String,
}
pub async fn correction(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<Correction>,
) -> Result<(StatusCode, Json<Value>)> {
    if !["correction", "supplement", "replacement", "anonymized"].contains(&input.kind.as_str()) {
        return Err(bad("Neplatný druh vazby."));
    }
    text(&input.reason, 2000)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let new_id = clone_draft(&mut tx, id).await?;
    sqlx::query("INSERT INTO document_relations(original_id,related_id,kind,actor_id,reason) VALUES($1,$2,$3,$4,$5)").bind(id).bind(new_id).bind(&input.kind).bind(admin.id).bind(&input.reason).execute(&mut *tx).await?;
    event(
        &mut tx,
        id,
        Some(admin.id),
        "related_draft_created",
        &input.reason,
        json!({"new_id":new_id,"kind":input.kind}),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({"id":new_id}))))
}
pub async fn clone_draft(conn: &mut PgConnection, id: i64) -> Result<i64> {
    let new_id:i64=sqlx::query_scalar("INSERT INTO documents(title,description,reference_number,issuer,category_id,document_date,source_published_on,created_at) SELECT title,description,reference_number,issuer,category_id,NULL,NULL,$2 FROM documents WHERE id=$1 RETURNING id").bind(id).bind(timestamp(OffsetDateTime::now_utc())).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
    sqlx::query("INSERT INTO attachments(document_id,name,content_type,size_bytes,sort_order,data,sha256,is_public) SELECT $2,name,content_type,size_bytes,sort_order,data,sha256,is_public FROM attachments WHERE document_id=$1 AND removed_at IS NULL AND data IS NOT NULL ORDER BY sort_order,id").bind(id).bind(new_id).execute(&mut *conn).await?;
    Ok(new_id)
}
pub async fn history(
    State(s): State<Backend>,
    _: Admin,
    Path(id): Path<i64>,
    Query(p): Query<super::Pagination>,
) -> Result<Json<Vec<Value>>> {
    let (limit, offset) = p.bounds()?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(e) FROM document_events e WHERE document_id=$1 ORDER BY id DESC LIMIT $2 OFFSET $3").bind(id).bind(limit).bind(offset).fetch_all(&s.pool).await?.into_iter().map(|mut value:Value|{prague_times(&mut value);value}).collect()))
}
pub async fn postings(pool: &PgPool, id: i64, public: bool) -> Result<Vec<Value>> {
    let sql = if public {
        "SELECT jsonb_build_object('id',p.id,'origin',origin,'certainty',certainty,'started_on',started_on,'started_at',started_at,'planned_removal_on',planned_removal_on,'removed_on',removed_on,'removed_at',removed_at,'corrections',coalesce((SELECT jsonb_agg(e.payload ORDER BY e.id) FROM document_events e WHERE e.document_id=p.document_id AND e.kind='evidence_corrected' AND (e.payload->>'posting_id')::bigint=p.id),'[]'::jsonb)) FROM document_postings p WHERE document_id=$1 ORDER BY p.id DESC LIMIT 100"
    } else {
        "SELECT to_jsonb(p) FROM document_postings p WHERE document_id=$1 ORDER BY id DESC LIMIT 100"
    };
    let mut rows: Vec<Value> = sqlx::query_scalar(sql).bind(id).fetch_all(pool).await?;
    for row in &mut rows {
        prague_times(row);
    }
    Ok(rows)
}

#[derive(Deserialize, Default)]
pub struct RevisionFilter {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub state: Option<String>,
}
pub async fn revisions(
    State(s): State<Backend>,
    _: Admin,
    Query(q): Query<RevisionFilter>,
) -> Result<Json<Vec<Value>>> {
    let (limit, offset) = super::Pagination {
        limit: q.limit,
        offset: q.offset,
    }
    .bounds()?;
    let state = q.state.unwrap_or_else(|| "pending".into());
    if !["pending", "accepted", "rejected"].contains(&state.as_str()) {
        return Err(bad("Neplatný stav revize."));
    }
    // Candidate bytes and whole audit histories never enter this list.
    Ok(Json(sqlx::query_scalar("SELECT jsonb_build_object('id',r.id,'source_key',r.source_key,'document_id',r.document_id,'fingerprint',r.fingerprint,'proposal',r.proposal,'first_seen_at',r.first_seen_at,'state',r.state,'accepted_document_id',r.accepted_document_id,'original',jsonb_build_object('title',d.title,'description',d.description,'attachments',coalesce((SELECT jsonb_agg(jsonb_build_object('name',a.name,'sha256',a.sha256) ORDER BY sort_order,a.id) FROM attachments a WHERE a.document_id=d.id),'[]'::jsonb)),'files',coalesce((SELECT jsonb_agg(jsonb_build_object('position',f.position,'name',f.name,'sha256',f.sha256,'size_bytes',f.size_bytes) ORDER BY position) FROM document_import_revision_files f WHERE f.revision_id=r.id),'[]'::jsonb)) FROM document_import_revisions r LEFT JOIN documents d ON d.id=r.document_id WHERE r.state=$1 ORDER BY r.first_seen_at DESC,r.id DESC LIMIT $2 OFFSET $3").bind(state).bind(limit).bind(offset).fetch_all(&s.pool).await?.into_iter().map(|mut value:Value|{prague_times(&mut value);value}).collect()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub action: String,
    pub reason: String,
}
pub async fn decide(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<Decision>,
) -> Result<Json<Value>> {
    text(&input.reason, 2000)?;
    if !["accept", "reject"].contains(&input.action.as_str()) {
        return Err(bad("Neplatné rozhodnutí."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let result: Option<i64> = sqlx::query_scalar("SELECT document_decide_revision($1,$2,$3,$4)")
        .bind(id)
        .bind(input.action)
        .bind(input.reason)
        .bind(admin.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":result})))
}
pub async fn revision_file(
    State(s): State<Backend>,
    _: Admin,
    Path((id, position)): Path<(i64, i32)>,
) -> Result<axum::response::Response> {
    use axum::{body::Body, http::header, response::IntoResponse};
    let (name,mime,bytes):(String,String,Vec<u8>)=sqlx::query_as("SELECT name,content_type,data FROM document_import_revision_files WHERE revision_id=$1 AND position=$2").bind(id).bind(position).fetch_optional(&s.pool).await?.ok_or_else(missing)?;
    let encoded = percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC);
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-store, private".into()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"revize\"; filename*=UTF-8''{encoded}"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'".into(),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

fn prague_times(value: &mut Value) {
    use time_tz::{OffsetDateTimeExt, timezones::db::europe::PRAGUE};
    for key in [
        "started_at",
        "removed_at",
        "occurred_at",
        "first_seen_at",
        "captured_at",
        "imported_at",
        "last_observed_at",
        "decided_at",
    ] {
        if let Some(v) = value.get_mut(key) {
            if let Some(parsed) = v.as_str().and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            }) {
                *v = json!(timestamp(parsed.to_timezone(PRAGUE)));
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkInput {
    pub related_id: i64,
    pub kind: String,
    pub reason: String,
}
pub async fn link(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<LinkInput>,
) -> Result<StatusCode> {
    text(&input.reason, 2000)?;
    if id == input.related_id
        || !["correction", "supplement", "replacement", "anonymized"].contains(&input.kind.as_str())
    {
        return Err(bad("Neplatná vazba."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=$1 OR id=$2")
        .bind(id)
        .bind(input.related_id)
        .fetch_one(&mut *tx)
        .await?;
    if n != 2 {
        return Err(missing());
    }
    sqlx::query("INSERT INTO document_relations(original_id,related_id,kind,actor_id,reason) VALUES($1,$2,$3,$4,$5)").bind(id).bind(input.related_id).bind(&input.kind).bind(admin.id).bind(&input.reason).execute(&mut *tx).await?;
    event(
        &mut tx,
        id,
        Some(admin.id),
        "document_linked",
        &input.reason,
        json!({"related_id":input.related_id,"kind":input.kind}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceCorrection {
    pub posting_id: i64,
    pub field: String,
    pub corrected_date: Option<Date>,
    pub reason: String,
}
pub async fn evidence_correction(
    State(s): State<Backend>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<EvidenceCorrection>,
) -> Result<StatusCode> {
    text(&input.reason, 2000)?;
    if !["started_on", "removed_on"].contains(&input.field.as_str()) {
        return Err(bad("Opravovat lze pouze výslovně označený evidenční údaj."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let (start, end): (Option<Date>, Option<Date>) = sqlx::query_as(
        "SELECT started_on,removed_on FROM document_postings WHERE id=$1 AND document_id=$2",
    )
    .bind(input.posting_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(missing)?;
    event(&mut tx,id,Some(admin.id),"evidence_corrected",&input.reason,json!({"posting_id":input.posting_id,"field":input.field,"original":if input.field=="started_on" {start}else{end},"corrected":input.corrected_date})).await?;
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}

pub async fn origin(pool: &PgPool, id: i64) -> Result<Option<Value>> {
    let mut result: Option<Value> = sqlx::query_scalar("WITH source_keys AS (
        SELECT source_key FROM legacy_sources WHERE document_id=$1
        UNION SELECT source_key FROM document_import_revisions WHERE accepted_document_id=$1
        ) SELECT jsonb_build_object('source_key',s.source_key,'source_url',s.source_url,'source_published_on',d.source_published_on,'imported_at',s.imported_at,'last_observed_at',o.last_observed_at,'mode',c.mode,'history',CASE WHEN d.lock_state='review' THEN 'uncertain' ELSE 'source' END)
        FROM source_keys k JOIN legacy_sources s USING(source_key)
        JOIN document_import_sources c ON c.id=s.source_id LEFT JOIN document_source_observations o ON o.source_key=s.source_key
        JOIN documents d ON d.id=$1 ORDER BY s.source_key LIMIT 1")
        .bind(id).fetch_optional(pool).await?;
    if let Some(value) = &mut result {
        prague_times(value);
    }
    Ok(result)
}

async fn posting_earliest(conn: &mut PgConnection, posting: i64) -> Result<Option<Date>> {
    let (day,review):(Option<Date>,String)=sqlx::query_as("SELECT p.started_on,d.review_json FROM document_postings p JOIN documents d ON d.id=p.document_id WHERE p.id=$1").bind(posting).fetch_optional(conn).await?.ok_or_else(missing)?;
    let review: crate::notice_policy::NoticeReview =
        serde_json::from_str(&review).map_err(|_| bad("Poškozené nastavení pravidla."))?;
    day.map(|d| review.earliest(d).map_err(bad))
        .transpose()
        .map(Option::flatten)
}
