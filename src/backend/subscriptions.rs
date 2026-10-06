use super::{
    Backend, Result, audit,
    auth::{self, ClientIp},
    bad,
};
use axum::{
    Form, Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

const VERIFICATION_COOLDOWN_SECONDS: i64 = 600;
const VERIFICATION_WINDOW_SECONDS: i64 = 3600;
const VERIFICATION_WINDOW_LIMIT: i64 = 3;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscribeInput {
    pub email: String,
    pub consent: ConsentAcceptance,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentAcceptance {
    pub fingerprint: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenInput {
    pub token: String,
}

pub async fn subscribe(
    State(s): State<Backend>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(input): Json<SubscribeInput>,
) -> Result<StatusCode> {
    auth::check_origin(&headers, &s)?;
    auth::throttle(
        &s,
        &format!("subscribe-ip:{ip}"),
        20,
        3600,
        OffsetDateTime::now_utc(),
    )
    .await?;
    request_subscription(&s, &input.email, &input.consent, OffsetDateTime::now_utc()).await?;
    Ok(StatusCode::ACCEPTED)
}
pub async fn request_subscription(
    s: &Backend,
    address: &str,
    consent: &ConsentAcceptance,
    now: OffsetDateTime,
) -> Result<()> {
    let notice = super::privacy::notice(s)?;
    if consent.fingerprint != notice.fingerprint {
        return Err(bad(
            "Informace o odběru se změnily. Obnovte stránku a přečtěte si aktuální znění.",
        ));
    }
    let address = auth::email(address)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let old: Option<(i64, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT id,verified_at,unsubscribed_at FROM subscribers WHERE email=$1")
            .bind(&address)
            .fetch_optional(&mut *tx)
            .await?;
    let proven: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM subscription_consents c JOIN subscribers s ON s.id=c.subscriber_id WHERE s.email=$1 AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL)")
        .bind(&address).fetch_one(&mut *tx).await?;
    if proven
        && old.as_ref().is_some_and(|(_, confirmed, unsubscribed)| {
            confirmed.is_some() && unsubscribed.is_none()
        })
    {
        tx.commit().await?;
        return Ok(());
    }
    // Count queued messages, not clicks. The write lock makes this persistent
    // sliding window and the following enqueue atomic across all processes.
    let (recent, last_activity): (i64, Option<i64>) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE m.created_at>$2),
         max(GREATEST(m.created_at,COALESCE(m.sent_at,m.created_at)))
         FROM mail_queue m JOIN subscribers s ON s.id=m.subscriber_id
         WHERE s.email=$1 AND m.purpose='verification'",
    )
    .bind(&address)
    .bind(now.unix_timestamp() - VERIFICATION_WINDOW_SECONDS)
    .fetch_one(&mut *tx)
    .await?;
    let waiting: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail_queue m
         JOIN subscribers s ON s.id=m.subscriber_id
         JOIN subscription_consents c ON c.id=m.consent_id AND c.subscriber_id=s.id
         WHERE s.email=$1 AND m.purpose='verification' AND m.sent_at IS NULL
           AND m.cancelled=FALSE AND m.body IS NOT NULL
           AND c.confirmed_at IS NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL
           AND EXISTS(SELECT 1 FROM subscription_tokens t WHERE t.subscriber_id=s.id
               AND t.consent_id=c.id AND t.purpose='verification' AND t.expires_at>$2))",
    )
    .bind(&address)
    .bind(now.unix_timestamp())
    .fetch_one(&mut *tx)
    .await?;
    if recent >= VERIFICATION_WINDOW_LIMIT
        || last_activity
            .is_some_and(|last| last > now.unix_timestamp() - VERIFICATION_COOLDOWN_SECONDS)
        || waiting
    {
        // Keep the same response for pending, confirmed and rate-limited addresses.
        tx.commit().await?;
        return Ok(());
    }
    let reusable_consent: Option<i64> = sqlx::query_scalar(
        "SELECT c.id FROM subscription_consents c JOIN subscribers s ON s.id=c.subscriber_id
         WHERE s.email=$1 AND c.notice_fingerprint=$2 AND c.confirmed_at IS NULL
           AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL
           AND EXISTS(SELECT 1 FROM subscription_tokens t WHERE t.subscriber_id=s.id
               AND t.consent_id=c.id AND t.purpose='verification' AND t.expires_at>$3)",
    )
    .bind(&address)
    .bind(&notice.fingerprint)
    .bind(now.unix_timestamp())
    .fetch_optional(&mut *tx)
    .await?;
    let id=sqlx::query_scalar::<_,i64>("INSERT INTO subscribers(email,retention_started_at) VALUES ($1,$2) ON CONFLICT(email) DO UPDATE SET verified_at=NULL,unsubscribed_at=NULL,retention_started_at=excluded.retention_started_at RETURNING id").bind(address).bind(now.unix_timestamp()).fetch_one(&mut *tx).await?;
    let consent_id = if let Some(consent_id) = reusable_consent {
        // A deliberate resend must not invalidate a link already in the inbox.
        consent_id
    } else {
        sqlx::query("UPDATE subscription_consents SET superseded_at=$1 WHERE subscriber_id=$2 AND confirmed_at IS NULL AND superseded_at IS NULL")
            .bind(now.unix_timestamp()).bind(id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO consent_notices(fingerprint,version,consent_text,privacy_json) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(&notice.fingerprint).bind(&notice.consent_version).bind(&notice.consent_text)
            .bind(serde_json::to_string(&notice.policy).expect("serializable privacy policy")).execute(&mut *tx).await?;
        let consent_id = sqlx::query_scalar::<_, i64>("INSERT INTO subscription_consents(subscriber_id,notice_fingerprint,requested_at) VALUES ($1,$2,$3) RETURNING id")
            .bind(id).bind(&notice.fingerprint).bind(now.unix_timestamp()).fetch_one(&mut *tx).await?;
        sqlx::query("DELETE FROM subscription_tokens WHERE subscriber_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        consent_id
    };
    sqlx::query(
        "UPDATE mail_queue SET cancelled=TRUE,body=NULL WHERE subscriber_id=$1 AND sent_at IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let secret = auth::token();
    sqlx::query("INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id) VALUES ($1,$2,'verification',$3,$4)")
        .bind(auth::hash(&secret))
        .bind(id)
        .bind(now.unix_timestamp() + 86400)
        .bind(consent_id)
        .execute(&mut *tx)
        .await?;
    let body = format!(
        "Dobrý den,\n\npotvrďte odběr nových dokumentů z webu Vyskeř na tomto odkazu:\n{}/odber/potvrdit?token={}\n\nOdkaz platí 24 hodin. Do potvrzení vám novinky chodit nebudou. Pokud jste o odběr nepožádali, zprávu ignorujte.\n\nZnění souhlasu ({}):\n{}\n\nInformace o soukromí: {}/ochrana-udaju\n\n{}",
        s.config.public_url,
        secret,
        notice.consent_version,
        notice.consent_text,
        s.config.public_url,
        notice.policy.controller_name
    );
    sqlx::query("INSERT INTO mail_queue(subscriber_id,purpose,subject,body,next_attempt_at,created_at,consent_id) VALUES ($1,'verification','Potvrzení odběru novinek z webu Vyskeř',$2,$3,$4,$5)")
        .bind(id).bind(body).bind(now.unix_timestamp()).bind(now.unix_timestamp()).bind(consent_id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        None,
        "subscription_requested",
        "subscriber",
        id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
fn validate_token(secret: &str) -> Result<()> {
    if secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad("Odkaz není platný nebo vypršel."));
    }
    Ok(())
}
pub async fn use_token(
    s: &Backend,
    secret: &str,
    confirm: bool,
    now: OffsetDateTime,
) -> Result<()> {
    validate_token(secret)?;
    let kind = if confirm {
        "verification"
    } else {
        "unsubscribe"
    };
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let (id, consent_id): (i64, Option<i64>) = sqlx::query_as(
        "DELETE FROM subscription_tokens WHERE hash=$1 AND purpose=$2 AND expires_at>$3 RETURNING subscriber_id,consent_id",
    )
    .bind(auth::hash(secret))
    .bind(kind)
    .bind(now.unix_timestamp())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| bad("Odkaz není platný nebo vypršel."))?;
    if confirm {
        let consent_id =
            consent_id.ok_or_else(|| bad("Požádejte o nový odběr a potvrďte aktuální souhlas."))?;
        let changed = sqlx::query("UPDATE subscription_consents SET confirmed_at=$1 WHERE id=$2 AND subscriber_id=$3 AND confirmed_at IS NULL AND withdrawn_at IS NULL AND superseded_at IS NULL")
            .bind(now.unix_timestamp()).bind(consent_id).bind(id).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(bad(
                "Souhlas již není možné potvrdit. Požádejte o nový odběr.",
            ));
        }
        sqlx::query("UPDATE subscribers SET verified_at=$1,unsubscribed_at=NULL WHERE id=$2")
            .bind(now.unix_timestamp())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE mail_queue SET cancelled=TRUE,body=NULL WHERE subscriber_id=$1 AND purpose='verification' AND sent_at IS NULL").bind(id).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM subscription_tokens WHERE subscriber_id=$1 AND purpose='verification'",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query("UPDATE subscription_consents SET withdrawn_at=$1 WHERE subscriber_id=$2 AND withdrawn_at IS NULL AND superseded_at IS NULL")
            .bind(now.unix_timestamp()).bind(id).execute(&mut *tx).await?;
        sqlx::query("UPDATE subscribers SET unsubscribed_at=$1 WHERE id=$2")
            .bind(now.unix_timestamp())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM subscription_tokens WHERE subscriber_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE mail_queue SET cancelled=TRUE,body=NULL WHERE subscriber_id=$1 AND sent_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    audit(&mut tx, None, kind, "subscriber", id, now).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn confirm(
    State(s): State<Backend>,
    Json(input): Json<TokenInput>,
) -> Result<StatusCode> {
    use_token(&s, &input.token, true, OffsetDateTime::now_utc()).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn unsubscribe(
    State(s): State<Backend>,
    Json(input): Json<TokenInput>,
) -> Result<StatusCode> {
    use_token(&s, &input.token, false, OffsetDateTime::now_utc()).await?;
    Ok(StatusCode::NO_CONTENT)
}

// Historical evidence must not depend on additions to today's runtime configuration.
#[derive(Deserialize)]
struct SubscriptionPolicySnapshot {
    version: String,
    controller_name: String,
    #[serde(default)]
    controller_address: String,
    controller_email: String,
    #[serde(default)]
    dpo_email: String,
    consent_evidence_legal_basis: String,
    processors: String,
    international_transfers: String,
    retention: SubscriptionRetentionSnapshot,
}
#[derive(Deserialize)]
struct SubscriptionRetentionSnapshot {
    pending_days: u16,
    withdrawn_days: u16,
    mail_days: u16,
    backup_days: u16,
}

// GET nikdy nemění odběr. Odkazy tak mohou bezpečně otevřít e-mailové skenery.
async fn landing(s: &Backend, secret: &str, confirm: bool) -> Result<Html<String>> {
    validate_token(secret)?;
    let (heading, action) = if confirm {
        ("Potvrdit odběr novinek", "/odber/potvrdit")
    } else {
        ("Odhlásit odběr novinek", "/odber/odhlasit")
    };
    let details = if confirm {
        let row: Option<(String, String, String)> = sqlx::query_as("SELECT n.version,n.consent_text,n.privacy_json FROM subscription_tokens t JOIN subscription_consents c ON c.id=t.consent_id JOIN consent_notices n ON n.fingerprint=c.notice_fingerprint WHERE t.hash=$1 AND t.purpose='verification' AND t.expires_at>$2 AND c.confirmed_at IS NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL")
            .bind(auth::hash(secret)).bind(OffsetDateTime::now_utc().unix_timestamp()).fetch_optional(&s.pool).await?;
        let (version, wording, json) =
            row.ok_or_else(|| bad("Odkaz není platný nebo vypršel. Požádejte o nový odběr."))?;
        let policy: SubscriptionPolicySnapshot = serde_json::from_str(&json)
            .map_err(|_| bad("Informace o souhlasu nejsou dostupné."))?;
        let controller_address = if policy.controller_address.trim().is_empty() {
            String::new()
        } else {
            format!(", {}", escape(&policy.controller_address))
        };
        let dpo = if policy.dpo_email.trim().is_empty() {
            String::new()
        } else {
            format!(" Pověřenec: {}.", escape(&policy.dpo_email))
        };
        let backups = if policy.retention.backup_days == 0 {
            "Vlastní zálohy databáze odběratelů nyní nevytváříme.".into()
        } else {
            format!("Zálohy: {} dní.", policy.retention.backup_days)
        };
        format!(
            "<p>{}</p><p>Verze souhlasu: {}. Informace o soukromí: {}.</p><details><summary>Informace platné při žádosti</summary><p>Správce: {}{controller_address}. Kontakt: {}.{dpo}</p><p>Účel: odběr nových dokumentů. Právní důvod: dobrovolný souhlas podle čl. 6 odst. 1 písm. a) GDPR. Souhlas lze kdykoli odvolat odkazem v každé zprávě.</p><p>Nepotvrzené žádosti: {} dní. Údaje po odhlášení: {} dní. Poštovní fronta: {} dní. {backups}</p><p>Důvod uchování dokladu: {}</p><p>Příjemci: {}</p><p>Předávání: {}</p><p>Práva: přístup, oprava, výmaz, omezení, podle právního důvodu přenositelnost a námitka. Stížnost lze podat u ÚOOÚ.</p></details>",
            escape(&wording),
            escape(&version),
            escape(&policy.version),
            escape(&policy.controller_name),
            escape(&policy.controller_email),
            policy.retention.pending_days,
            policy.retention.withdrawn_days,
            policy.retention.mail_days,
            escape(&policy.consent_evidence_legal_basis),
            escape(&policy.processors),
            escape(&policy.international_transfers)
        )
    } else {
        "<p>Odhlášením odvoláte souhlas s dalším zasíláním novinek. Přihlášení k účtu není potřeba.</p>".into()
    };
    Ok(Html(format!(
        "<!doctype html><html lang=\"cs\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"robots\" content=\"noindex\"><title>{heading} · Vyskeř</title><body><main><h1>{heading}</h1><p>Web Vyskeř</p>{details}<form method=\"post\" action=\"{action}\"><input type=\"hidden\" name=\"token\" value=\"{secret}\"><button type=\"submit\">{heading}</button></form></main></body></html>"
    )))
}
pub async fn confirmation_page(
    State(s): State<Backend>,
    Query(input): Query<TokenInput>,
) -> Result<Html<String>> {
    landing(&s, &input.token, true).await
}
pub async fn unsubscribe_page(
    State(s): State<Backend>,
    Query(input): Query<TokenInput>,
) -> Result<Html<String>> {
    landing(&s, &input.token, false).await
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub async fn confirmation_form(
    State(s): State<Backend>,
    Form(input): Form<TokenInput>,
) -> Result<Html<&'static str>> {
    use_token(&s, &input.token, true, OffsetDateTime::now_utc()).await?;
    Ok(Html(
        "<!doctype html><html lang=cs><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Odběr potvrzen · Vyskeř</title><h1>Odběr je potvrzený.</h1><p>Nové dokumenty vám nyní přijdou e-mailem.</p><a href=/>Zpět na web Vyskeř</a></html>",
    ))
}
pub async fn unsubscribe_form(
    State(s): State<Backend>,
    Form(input): Form<TokenInput>,
) -> Result<Html<&'static str>> {
    use_token(&s, &input.token, false, OffsetDateTime::now_utc()).await?;
    Ok(Html(
        "<!doctype html><html lang=cs><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Odběr odhlášen · Vyskeř</title><h1>Odběr je odhlášený.</h1><p>Další novinky vám posílat nebudeme.</p><a href=/>Zpět na web Vyskeř</a></html>",
    ))
}
