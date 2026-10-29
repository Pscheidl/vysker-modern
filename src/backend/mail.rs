use super::{Backend, Result, auth};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};
use sqlx::PgConnection;
use time::OffsetDateTime;
use time_tz::{OffsetDateTimeExt, timezones::db::europe::PRAGUE};

pub fn transport(s: &Backend) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
    s.config.validate_smtp_capture()?;
    let mut builder = match s.config.smtp_tls.as_str() {
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&s.config.smtp_host),
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&s.config.smtp_host)?,
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&s.config.smtp_host)?,
    }
    .port(s.config.smtp_port)
    .timeout(Some(std::time::Duration::from_secs(20)));
    if let (Some(user), Some(password)) = (&s.config.smtp_username, &s.config.smtp_password) {
        builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
    }
    Ok(builder.build())
}

/// Refresh settings before claiming a message and reuse the SMTP pool until
/// its credentials or sender change. A settings failure leaves queues untouched.
pub struct Worker {
    state: Backend,
    smtp: AsyncSmtpTransport<Tokio1Executor>,
}

impl Worker {
    pub fn new(state: &Backend) -> anyhow::Result<Self> {
        Ok(Self {
            state: state.clone(),
            smtp: transport(state)?,
        })
    }

    pub async fn deliver_one(
        &mut self,
        state: &Backend,
        now: OffsetDateTime,
    ) -> anyhow::Result<bool> {
        let config = super::mail_settings::effective_config(state).await?;
        let previous = &self.state.config;
        if config.smtp_host != previous.smtp_host
            || config.smtp_port != previous.smtp_port
            || config.smtp_tls != previous.smtp_tls
            || config.smtp_username != previous.smtp_username
            || config.smtp_password != previous.smtp_password
            || config.email_from != previous.email_from
        {
            let mut configured = state.clone();
            configured.config = std::sync::Arc::new(config);
            self.smtp = transport(&configured)?;
            self.state = configured;
        }
        deliver_one(&self.state, &self.smtp, now).await
    }
}

pub enum PublicationTarget {
    Notice(i64),
    Document(i64),
    Event(i64),
}

/// Snapshot recipients in the same transaction that makes the content public.
/// Both importers and native publication hand delivery to the durable outbox.
pub async fn enqueue_publication(
    conn: &mut PgConnection,
    target: PublicationTarget,
    now: OffsetDateTime,
) -> Result<()> {
    let (notice_id, document_id, event_id) = match target {
        PublicationTarget::Notice(id) => (Some(id), None, None),
        PublicationTarget::Document(id) => (None, Some(id), None),
        PublicationTarget::Event(id) => (None, None, Some(id)),
    };
    sqlx::query(
        "INSERT INTO publication_outbox (notice_id,document_id,event_id,subscriber_id,consent_id,created_at)
         SELECT $1,$2,$3,s.id,c.id,$4 FROM subscribers s
         JOIN subscription_consents c ON c.subscriber_id=s.id
         WHERE s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
           AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL
           AND subscription_matches_publication(s.id,$1,$2,$3)
         ORDER BY s.id,c.id ON CONFLICT DO NOTHING",
    )
    .bind(notice_id)
    .bind(document_id)
    .bind(event_id)
    .bind(now.unix_timestamp())
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct PublicationRequest {
    id: i64,
    notice_id: Option<i64>,
    document_id: Option<i64>,
    event_id: Option<i64>,
    title: String,
    allowed: bool,
}

const DIGEST_FOOTER: &str = "\n\nOdběr lze odhlásit zde:\n";

#[derive(sqlx::FromRow)]
struct EventPublication {
    title: String,
    location: String,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    all_day: bool,
    start_time_known: bool,
    end_time_known: bool,
    end_date_known: bool,
    repeat_every_14_days: bool,
    repeat_until: Option<time::Date>,
}

impl EventPublication {
    fn line(self, id: i64, public_url: &str) -> String {
        // Share the public calendar's labels so legacy placeholder times never
        // become advertised start or end times in email.
        let event = crate::events::Event {
            id,
            title: self.title,
            location: self.location,
            starts_at: super::timestamp(self.starts_at.to_timezone(PRAGUE)),
            ends_at: super::timestamp(self.ends_at.to_timezone(PRAGUE)),
            all_day: self.all_day,
            start_time_known: self.start_time_known,
            end_time_known: self.end_time_known,
            end_date_known: self.end_date_known,
            repeat_every_14_days: self.repeat_every_14_days,
            repeat_until: self.repeat_until.map(|date| date.to_string()),
            ..Default::default()
        };
        let mut lines = vec![event.title.replace(['\r', '\n'], " ")];
        if event.all_day && event.starts_at[..10] == event.ends_at[..10] {
            lines.push(format!("Termín: {}", event.start_label()));
        } else {
            lines.push(format!("Začátek: {}", event.start_label()));
            lines.push(format!("Konec: {}", event.end_label()));
        }
        if let Some(recurrence) = event.recurrence_label() {
            lines.push(recurrence);
        }
        if !event.location.trim().is_empty() {
            lines.push(format!(
                "Místo: {}",
                event.location.replace(['\r', '\n'], " ")
            ));
        }
        lines.push(format!("{public_url}/kalendar/{id}"));
        lines.join("\n")
    }
}

async fn publication_line(
    conn: &mut PgConnection,
    request: &PublicationRequest,
    public_url: &str,
    now: OffsetDateTime,
) -> Result<String> {
    if let Some(id) = request.event_id {
        let event = sqlx::query_as::<_, EventPublication>(
            "SELECT title,location,starts_at,ends_at,all_day,start_time_known,end_time_known,
                end_date_known,repeat_every_14_days,repeat_until FROM events WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
        return Ok(event.line(id, public_url));
    }
    let (title, route, id) = if let Some(id) = request.notice_id {
        let record =
            sqlx::query_as::<_, super::notices::NoticeRecord>("SELECT * FROM notices WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *conn)
                .await?;
        (record.into_public(now).title, "uredni-deska", id)
    } else {
        (
            request.title.clone(),
            "dokumenty",
            request.document_id.expect("publication target"),
        )
    };
    Ok(format!(
        "{}\n{public_url}/{route}/{id}",
        title.replace(['\r', '\n'], " ")
    ))
}

fn digest_content(lines: &[String], footer: &str, has_events: bool) -> (String, String) {
    let (subject, description) = if has_events {
        ("Souhrn novinek", "tyto novinky")
    } else {
        ("Souhrn nových dokumentů", "tyto dokumenty")
    };
    (
        format!("Vyskeř: {subject} ({})", lines.len()),
        format!(
            "Dobrý den,\n\nna webu Vyskeř byly zveřejněny {description}:\n\n{}{DIGEST_FOOTER}{footer}",
            lines.join("\n\n")
        ),
    )
}

/// Atomically prepare the complete due set for one original recipient and consent.
/// The time boundary schedules work only. Late commits remain in the outbox.
pub async fn prepare_publications(s: &Backend, now: OffsetDateTime) -> Result<bool> {
    let cutoff = now.unix_timestamp().div_euclid(3600) * 3600;
    // The common write lock serializes preparation with publication and consent edits.
    // SMTP runs outside it. Processing one recipient bounds each transaction's scope.
    let mut tx = crate::db::begin_write(&s.pool).await?;
    if super::privacy::policy_in(s, &mut tx).await?.is_none() {
        return Ok(false);
    }
    let recipient: Option<(i64, i64)> = sqlx::query_as(
        "SELECT o.subscriber_id,o.consent_id FROM publication_outbox o
         JOIN subscribers s ON s.id=o.subscriber_id
         WHERE o.created_at<$1 AND s.next_digest_at<=$1
         ORDER BY o.created_at,o.id LIMIT 1",
    )
    .bind(cutoff)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((recipient, consent_id)) = recipient else {
        return Ok(false);
    };
    let publications: Vec<PublicationRequest> = sqlx::query_as(
        "SELECT o.id,o.notice_id,o.document_id,o.event_id,coalesce(n.title,d.title,e.title) AS title,
            (s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
             AND c.subscriber_id=s.id AND c.confirmed_at IS NOT NULL
             AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL
             AND subscription_matches_publication(s.id,o.notice_id,o.document_id,o.event_id)
             AND (coalesce(n.status IN ('published','archived','withdrawn'),FALSE)
                  OR coalesce(d.status='published',FALSE)
                  OR coalesce(e.published AND NOT e.cancelled,FALSE))
             AND NOT EXISTS (SELECT 1 FROM mail_queue m WHERE m.deduplication_key=
                 concat(CASE WHEN o.notice_id IS NOT NULL THEN 'notice'
                             WHEN o.document_id IS NOT NULL THEN 'document' ELSE 'event' END,
                        ':',coalesce(o.notice_id,o.document_id,o.event_id),':',o.subscriber_id))
             AND NOT EXISTS (SELECT 1 FROM mail_publication_items i
                 WHERE i.subscriber_id=o.subscriber_id
                   AND (i.notice_id=o.notice_id OR i.document_id=o.document_id OR i.event_id=o.event_id))) AS allowed
         FROM publication_outbox o
         JOIN subscribers s ON s.id=o.subscriber_id
         JOIN subscription_consents c ON c.id=o.consent_id
         LEFT JOIN notices n ON n.id=o.notice_id
         LEFT JOIN documents d ON d.id=o.document_id
         LEFT JOIN events e ON e.id=o.event_id
         WHERE o.subscriber_id=$1 AND o.consent_id=$2 AND o.created_at<$3
         ORDER BY o.created_at,o.id FOR UPDATE OF o",
    )
    .bind(recipient)
    .bind(consent_id)
    .bind(cutoff)
    .fetch_all(&mut *tx)
    .await?;
    let mut lines = Vec::new();
    for request in publications.iter().filter(|r| r.allowed) {
        lines.push(publication_line(&mut tx, request, &s.config.public_url, now).await?);
    }
    if !lines.is_empty() {
        let secret = auth::token();
        sqlx::query(
            "INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id)
             VALUES ($1,$2,'unsubscribe',$3,$4)",
        )
        .bind(auth::hash(&secret))
        .bind(recipient)
        .bind(i64::MAX)
        .bind(consent_id)
        .execute(&mut *tx)
        .await?;
        let footer = format!(
            "{}/odber/odhlasit?token={secret}\n\nTémata odběru si můžete vybrat zde:\n{}/odber/nastaveni?token={secret}",
            s.config.public_url, s.config.public_url
        );
        let has_events = publications
            .iter()
            .any(|r| r.allowed && r.event_id.is_some());
        let (subject, body) = digest_content(&lines, &footer, has_events);
        let mail_id: i64 = sqlx::query_scalar(
            "INSERT INTO mail_queue(subscriber_id,purpose,deduplication_key,subject,body,
                next_attempt_at,created_at,consent_id,digest_cutoff)
             VALUES ($1,'document',$2,$3,$4,$5,$5,$6,$7) RETURNING id",
        )
        .bind(recipient)
        .bind(format!("digest:{recipient}:{consent_id}:{cutoff}"))
        .bind(subject)
        .bind(body)
        .bind(now.unix_timestamp())
        .bind(consent_id)
        .bind(cutoff)
        .fetch_one(&mut *tx)
        .await?;
        for request in publications.iter().filter(|r| r.allowed) {
            sqlx::query(
                "INSERT INTO mail_publication_items(mail_id,subscriber_id,notice_id,document_id,event_id)
                 VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(mail_id)
            .bind(recipient)
            .bind(request.notice_id)
            .bind(request.document_id)
            .bind(request.event_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE subscribers SET next_digest_at=$1 WHERE id=$2")
            .bind(cutoff + 3600)
            .bind(recipient)
            .execute(&mut *tx)
            .await?;
    }
    // Delete exactly the selected set, including ineligible and already recorded items.
    let ids: Vec<i64> = publications.iter().map(|r| r.id).collect();
    sqlx::query("DELETE FROM publication_outbox WHERE id=ANY($1)")
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Rebuild only from this message's recorded membership, never from newer outbox rows.
/// Once suppressed, an item stays suppressed even if preferences later change again.
async fn refresh_digest(
    s: &Backend,
    message: &mut Pending,
    lease: &str,
    now: OffsetDateTime,
) -> Result<bool> {
    let Some(footer) = message
        .body
        .as_deref()
        .and_then(|body| body.split_once(DIGEST_FOOTER))
        .map(|(_, footer)| footer.to_owned())
    else {
        return Ok(false);
    };
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail_queue m
         JOIN subscribers s ON s.id=m.subscriber_id
         JOIN subscription_consents c ON c.id=m.consent_id AND c.subscriber_id=s.id
         WHERE m.id=$1 AND m.lock_token=$2 AND m.cancelled=FALSE AND m.sent_at IS NULL
           AND s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
           AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL)",
    )
    .bind(message.id)
    .bind(lease)
    .fetch_one(&mut *tx)
    .await?;
    if !valid {
        return Ok(false);
    }
    let items: Vec<PublicationRequest> = sqlx::query_as(
        "SELECT i.id,i.notice_id,i.document_id,i.event_id,coalesce(n.title,d.title,e.title) AS title,
            (subscription_matches_publication(i.subscriber_id,i.notice_id,i.document_id,i.event_id)
             AND (coalesce(n.status IN ('published','archived','withdrawn'),FALSE)
                  OR coalesce(d.status='published',FALSE)
                  OR coalesce(e.published AND NOT e.cancelled,FALSE))) AS allowed
         FROM mail_publication_items i
         LEFT JOIN notices n ON n.id=i.notice_id LEFT JOIN documents d ON d.id=i.document_id
         LEFT JOIN events e ON e.id=i.event_id
         WHERE i.mail_id=$1 AND i.subscriber_id=$2 AND i.suppressed=FALSE ORDER BY i.id",
    )
    .bind(message.id)
    .bind(message.subscriber_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut lines = Vec::new();
    for item in &items {
        if item.allowed {
            lines.push(publication_line(&mut tx, item, &s.config.public_url, now).await?);
        } else {
            sqlx::query("UPDATE mail_publication_items SET suppressed=TRUE WHERE id=$1")
                .bind(item.id)
                .execute(&mut *tx)
                .await?;
        }
    }
    if !lines.is_empty() {
        let has_events = items.iter().any(|r| r.allowed && r.event_id.is_some());
        let (subject, body) = digest_content(&lines, &footer, has_events);
        sqlx::query("UPDATE mail_queue SET subject=$1,body=$2 WHERE id=$3 AND lock_token=$4")
            .bind(&subject)
            .bind(&body)
            .bind(message.id)
            .bind(lease)
            .execute(&mut *tx)
            .await?;
        message.subject = subject;
        message.body = Some(body);
    }
    tx.commit().await?;
    Ok(!lines.is_empty())
}

#[derive(sqlx::FromRow)]
struct Pending {
    id: i64,
    subscriber_id: i64,
    purpose: String,
    subject: String,
    body: Option<String>,
    attempts: i64,
    consent_id: Option<i64>,
    deduplication_key: Option<String>,
    digest_cutoff: Option<i64>,
}

impl Pending {
    /// Publication keys retain their type and recipient across worker restarts.
    fn publication_target(&self) -> Option<PublicationTarget> {
        let mut parts = self.deduplication_key.as_deref()?.split(':');
        let kind = parts.next()?;
        let id = parts.next()?.parse::<i64>().ok()?;
        let recipient = parts.next()?.parse::<i64>().ok()?;
        if parts.next().is_some() || id <= 0 || recipient != self.subscriber_id {
            return None;
        }
        match kind {
            "notice" => Some(PublicationTarget::Notice(id)),
            "document" => Some(PublicationTarget::Document(id)),
            "event" => Some(PublicationTarget::Event(id)),
            _ => None,
        }
    }
}

async fn matches_preferences(s: &Backend, message: &Pending) -> anyhow::Result<bool> {
    if message.purpose != "document" {
        return Ok(true);
    }
    let (notice_id, document_id, event_id) = match message.publication_target() {
        Some(PublicationTarget::Notice(id)) => (Some(id), None, None),
        Some(PublicationTarget::Document(id)) => (None, Some(id), None),
        Some(PublicationTarget::Event(id)) => (None, None, Some(id)),
        None => return Ok(false),
    };
    Ok(
        sqlx::query_scalar("SELECT subscription_matches_publication($1,$2,$3,$4)")
            .bind(message.subscriber_id)
            .bind(notice_id)
            .bind(document_id)
            .bind(event_id)
            .fetch_one(&s.pool)
            .await?,
    )
}

/// Trvalá fronta s časově omezeným zámkem. Pád procesu zprávu neztratí.
/// SMTP doručení je alespoň jednou, stejný Message-ID usnadňuje deduplikaci.
pub async fn deliver_one(
    s: &Backend,
    smtp: &AsyncSmtpTransport<Tokio1Executor>,
    now: OffsetDateTime,
) -> anyhow::Result<bool> {
    if super::recovery::deliver_one(s, smtp, now).await? {
        return Ok(true);
    }
    if super::reports::deliver_one(s, smtp, now).await? {
        return Ok(true);
    }
    if super::privacy::policy(s)
        .await
        .map_err(|error| anyhow::anyhow!(error.1))?
        .is_none()
    {
        return Ok(false);
    }
    let prepared = prepare_publications(s, now)
        .await
        .map_err(|error| anyhow::anyhow!(error.1))?;
    let lease = auth::token();
    let message:Option<Pending>=sqlx::query_as("UPDATE mail_queue SET lock_token=$1,locked_until=$2,attempts=attempts+1 WHERE id=(SELECT id FROM mail_queue WHERE sent_at IS NULL AND cancelled=FALSE AND next_attempt_at<=$3 AND locked_until<=$4 ORDER BY CASE WHEN purpose='verification' THEN 0 ELSE 1 END,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,subscriber_id,purpose,subject,body,attempts,consent_id,deduplication_key,digest_cutoff")
        .bind(&lease).bind(now.unix_timestamp()+300).bind(now.unix_timestamp()).bind(now.unix_timestamp()).fetch_optional(&s.pool).await?;
    let Some(mut message) = message else {
        return Ok(prepared);
    };
    let (address, confirmed, unsubscribed): (String, Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT email,verified_at,unsubscribed_at FROM subscribers WHERE id=$1")
            .bind(message.subscriber_id)
            .fetch_one(&s.pool)
            .await?;
    let verification_valid = if message.purpose == "verification" {
        let secret = message
            .body
            .as_deref()
            .and_then(|body| body.split("token=").nth(1))
            .and_then(|value| value.split_whitespace().next())
            .unwrap_or("");
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM subscription_tokens WHERE hash=$1 AND subscriber_id=$2 AND purpose='verification' AND expires_at>$3)")
            .bind(auth::hash(secret)).bind(message.subscriber_id).bind(now.unix_timestamp()).fetch_one(&s.pool).await?
    } else {
        true
    };
    let still_pending: bool =
        sqlx::query_scalar("SELECT cancelled=FALSE FROM mail_queue WHERE id=$1 AND lock_token=$2")
            .bind(message.id)
            .bind(&lease)
            .fetch_optional(&s.pool)
            .await?
            .unwrap_or(false);
    let allowed = still_pending
        && verification_valid
        && sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM subscription_consents WHERE id=$1 AND subscriber_id=$2 AND withdrawn_at IS NULL AND superseded_at IS NULL AND ($3='verification' OR confirmed_at IS NOT NULL))")
            .bind(message.consent_id).bind(message.subscriber_id).bind(&message.purpose).fetch_one(&s.pool).await?
        && unsubscribed.is_none()
        && if message.purpose == "document" {
            confirmed.is_some() && if message.digest_cutoff.is_some() {
                refresh_digest(s, &mut message, &lease, now).await
                    .map_err(|error| anyhow::anyhow!(error.1))?
            } else {
                matches_preferences(s, &message).await?
            }
        } else {
            confirmed.is_none()
        };
    if !allowed || message.body.is_none() {
        sqlx::query(
            "UPDATE mail_queue SET cancelled=TRUE,body=NULL,lock_token=NULL,locked_until=0 WHERE id=$1 AND lock_token=$2",
        )
        .bind(message.id)
        .bind(&lease)
        .execute(&s.pool)
        .await?;
        return Ok(true);
    }
    let outcome = async {
        let email = Message::builder()
            .from(s.config.email_from.parse()?)
            .to(address.parse()?)
            .message_id(Some(format!(
                "vysker-post-{}@{}",
                message.id,
                url::Url::parse(&s.config.public_url)?
                    .host_str()
                    .unwrap_or("localhost")
            )))
            .subject(&message.subject)
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(message.body.unwrap_or_default())?;
        smtp.send(email).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    match outcome {
        Ok(()) => {
            sqlx::query("UPDATE mail_queue SET sent_at=$1,body=NULL,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3")
                .bind(now.unix_timestamp()).bind(message.id).bind(&lease).execute(&s.pool).await?;
        }
        Err(_) => {
            tracing::warn!(
                message_id = message.id,
                "SMTP selhalo, zpráva zůstává ve frontě"
            );
            let delay = (30_i64 * 2_i64.pow(message.attempts.min(10) as u32)).min(21600);
            sqlx::query(
                "UPDATE mail_queue SET next_attempt_at=$1,lock_token=NULL,locked_until=0 WHERE id=$2 AND lock_token=$3",
            )
            .bind(now.unix_timestamp() + delay)
            .bind(message.id)
            .bind(&lease)
            .execute(&s.pool)
            .await?;
        }
    }
    Ok(true)
}
