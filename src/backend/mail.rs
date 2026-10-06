use super::{Backend, Result, auth};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

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
}

/// Snapshot recipients in the same transaction that makes the content public.
/// Both importers and native publication hand delivery to the durable outbox.
pub async fn enqueue_publication(
    conn: &mut PgConnection,
    target: PublicationTarget,
    now: OffsetDateTime,
) -> Result<()> {
    let (notice_id, document_id) = match target {
        PublicationTarget::Notice(id) => (Some(id), None),
        PublicationTarget::Document(id) => (None, Some(id)),
    };
    sqlx::query(
        "INSERT INTO publication_outbox (notice_id,document_id,subscriber_id,consent_id,created_at)
         SELECT $1,$2,s.id,c.id,$3 FROM subscribers s
         JOIN subscription_consents c ON c.subscriber_id=s.id
         WHERE s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
           AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL
           AND subscription_matches_publication(s.id,$1,$2)
         ORDER BY s.id,c.id ON CONFLICT DO NOTHING",
    )
    .bind(notice_id)
    .bind(document_id)
    .bind(now.unix_timestamp())
    .execute(conn)
    .await?;
    Ok(())
}

struct Publication<'a> {
    kind: &'a str,
    id: i64,
    title: &'a str,
    path: &'a str,
}

async fn enqueue_recipient(
    conn: &mut PgConnection,
    s: &Backend,
    publication: &Publication<'_>,
    (recipient, consent_id): (i64, i64),
    created_at: i64,
) -> Result<()> {
    let Some(policy) = &s.config.privacy else {
        return Ok(());
    };
    let Publication {
        kind,
        id,
        title,
        path,
    } = publication;
    let key = format!("{kind}:{id}:{recipient}");
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mail_queue WHERE deduplication_key=$1)")
            .bind(&key)
            .fetch_one(&mut *conn)
            .await?;
    if exists {
        return Ok(());
    }
    let secret = auth::token();
    sqlx::query(
            "INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id) VALUES ($1,$2,'unsubscribe',$3,$4)",
        )
        .bind(auth::hash(&secret))
        .bind(recipient)
        // The link remains usable for the lifetime of this consent.
        .bind(i64::MAX)
        .bind(consent_id)
        .execute(&mut *conn)
        .await?;
    let body = format!(
        "Dobrý den,\n\nna webu Vyskeř byl zveřejněn nový dokument:\n{title}\n\n{}{path}\n\nOdběr lze odhlásit zde:\n{}/odber/odhlasit?token={secret}\n\nKategorie a obecné dokumenty si můžete vybrat zde:\n{}/odber/nastaveni?token={secret}\n\n{}",
        s.config.public_url, s.config.public_url, s.config.public_url, policy.controller_name
    );
    sqlx::query("INSERT INTO mail_queue(subscriber_id,purpose,deduplication_key,subject,body,next_attempt_at,created_at,consent_id) VALUES ($1,'document',$2,$3,$4,$5,$6,$7)")
            .bind(recipient).bind(key).bind(format!("Vyskeř: {}",title.replace(['\r','\n']," "))).bind(body).bind(created_at).bind(created_at).bind(consent_id).execute(&mut *conn).await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct PublicationRequest {
    id: i64,
    subscriber_id: i64,
    consent_id: i64,
    created_at: i64,
    kind: String,
    entity_id: i64,
    title: String,
    allowed: bool,
}

/// Atomically transfer a bounded outbox batch to the SMTP queue.
/// Keep the original recipient and consent even if delivery starts much later.
pub async fn prepare_publications(s: &Backend, now: OffsetDateTime) -> Result<bool> {
    if s.config.privacy.is_none() {
        return Ok(false);
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let publications: Vec<PublicationRequest> = sqlx::query_as(
        "SELECT o.id,o.subscriber_id,o.consent_id,o.created_at,
            CASE WHEN o.notice_id IS NOT NULL THEN 'notice' ELSE 'document' END AS kind,
            coalesce(o.notice_id,o.document_id) AS entity_id,coalesce(n.title,d.title) AS title,
            (s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
             AND c.subscriber_id=s.id AND c.confirmed_at IS NOT NULL
             AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL
             AND subscription_matches_publication(s.id,o.notice_id,o.document_id)
             AND (coalesce(n.status IN ('published','archived','withdrawn'),FALSE)
                  OR coalesce(d.status='published',FALSE))) AS allowed
         FROM publication_outbox o
         JOIN subscribers s ON s.id=o.subscriber_id
         JOIN subscription_consents c ON c.id=o.consent_id
         LEFT JOIN notices n ON n.id=o.notice_id
         LEFT JOIN documents d ON d.id=o.document_id
         ORDER BY o.id LIMIT 100 FOR UPDATE OF o",
    )
    .fetch_all(&mut *tx)
    .await?;
    for request in &publications {
        if request.allowed {
            let title = if request.kind == "notice" {
                sqlx::query_as::<_, super::notices::NoticeRecord>(
                    "SELECT * FROM notices WHERE id=$1",
                )
                .bind(request.entity_id)
                .fetch_one(&mut *tx)
                .await?
                .into_public(now)
                .title
            } else {
                request.title.clone()
            };
            let route = if request.kind == "notice" {
                "uredni-deska"
            } else {
                "dokumenty"
            };
            let path = format!("/{route}/{}", request.entity_id);
            let publication = Publication {
                kind: &request.kind,
                id: request.entity_id,
                title: &title,
                path: &path,
            };
            enqueue_recipient(
                &mut tx,
                s,
                &publication,
                (request.subscriber_id, request.consent_id),
                request.created_at,
            )
            .await?;
        }
        sqlx::query("DELETE FROM publication_outbox WHERE id=$1")
            .bind(request.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(!publications.is_empty())
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
            _ => None,
        }
    }
}

async fn matches_preferences(s: &Backend, message: &Pending) -> anyhow::Result<bool> {
    if message.purpose != "document" {
        return Ok(true);
    }
    let (notice_id, document_id) = match message.publication_target() {
        Some(PublicationTarget::Notice(id)) => (Some(id), None),
        Some(PublicationTarget::Document(id)) => (None, Some(id)),
        None => return Ok(false),
    };
    Ok(
        sqlx::query_scalar("SELECT subscription_matches_publication($1,$2,$3)")
            .bind(message.subscriber_id)
            .bind(notice_id)
            .bind(document_id)
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
    if s.config.privacy.is_none() {
        return Ok(false);
    }
    let prepared = prepare_publications(s, now)
        .await
        .map_err(|error| anyhow::anyhow!(error.1))?;
    let lease = auth::token();
    let message:Option<Pending>=sqlx::query_as("UPDATE mail_queue SET lock_token=$1,locked_until=$2,attempts=attempts+1 WHERE id=(SELECT id FROM mail_queue WHERE sent_at IS NULL AND cancelled=FALSE AND next_attempt_at<=$3 AND locked_until<=$4 ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,subscriber_id,purpose,subject,body,attempts,consent_id,deduplication_key")
        .bind(&lease).bind(now.unix_timestamp()+300).bind(now.unix_timestamp()).bind(now.unix_timestamp()).fetch_optional(&s.pool).await?;
    let Some(message) = message else {
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
            confirmed.is_some() && matches_preferences(s, &message).await?
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
