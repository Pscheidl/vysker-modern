//! Bearer-link settings for an already confirmed subscription.
use super::{Backend, Result, audit, auth, bad, subscriptions};
use crate::catalog::SubscriptionPreferences;
use axum::{
    Form,
    extract::{Query, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
};
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(sqlx::FromRow)]
struct CurrentSubscription {
    id: i64,
    all_notice_categories: bool,
    notice_category_ids: Vec<i64>,
    uncategorized_notices: bool,
    documents: bool,
}

async fn current(
    conn: &mut sqlx::PgConnection,
    token: &str,
    now: OffsetDateTime,
) -> Result<CurrentSubscription> {
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(bad("Odkaz není platný nebo vypršel."));
    }
    sqlx::query_as(
        "SELECT s.id,s.all_notice_categories,s.notice_category_ids,s.uncategorized_notices,s.documents
         FROM subscribers s JOIN subscription_tokens t ON t.subscriber_id=s.id
         JOIN subscription_consents c ON c.id=t.consent_id AND c.subscriber_id=s.id
         WHERE t.hash=$1 AND t.purpose='unsubscribe' AND t.expires_at>$2
           AND s.verified_at IS NOT NULL AND s.unsubscribed_at IS NULL
           AND c.confirmed_at IS NOT NULL AND c.withdrawn_at IS NULL AND c.superseded_at IS NULL",
    )
    .bind(auth::hash(token))
    .bind(now.unix_timestamp())
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| bad("Odkaz není platný nebo vypršel."))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn checkbox(name: &str, label: &str, checked: bool) -> String {
    format!(
        "<label><input type=\"checkbox\" name=\"{}\" value=\"on\"{}> {}</label>",
        escape(name),
        if checked { " checked" } else { "" },
        escape(label)
    )
}

async fn settings_page(
    s: &Backend,
    token: &str,
    saved: bool,
    attempted: Option<&SubscriptionPreferences>,
    error: Option<&str>,
) -> Result<Html<String>> {
    let mut conn = s.pool.acquire().await?;
    let current = current(&mut conn, token, OffsetDateTime::now_utc()).await?;
    let categories: Vec<(i64, String)> =
        sqlx::query_as("SELECT id,name FROM categories ORDER BY sort_order,id")
            .fetch_all(&mut *conn)
            .await?;
    let preferences = attempted.cloned().unwrap_or(SubscriptionPreferences {
        all_notice_categories: current.all_notice_categories,
        notice_category_ids: current.notice_category_ids,
        uncategorized_notices: current.uncategorized_notices,
        documents: current.documents,
    });
    let all = checkbox(
        "all_notice_categories",
        "Všechny kategorie úřední desky, i nově přidané",
        preferences.all_notice_categories,
    );
    let choices = categories
        .iter()
        .map(|(id, name)| {
            checkbox(
                &format!("category_{id}"),
                name,
                preferences.notice_category_ids.contains(id),
            )
        })
        .collect::<String>();
    let uncategorized = checkbox(
        "uncategorized_notices",
        "Úřední deska bez kategorie",
        preferences.uncategorized_notices,
    );
    let documents = checkbox("documents", "Obecné dokumenty", preferences.documents);
    let status = if saved {
        "<p role=\"status\">Výběr je uložený. Další novinky vám budeme posílat podle tohoto nastavení.</p>"
    } else {
        ""
    };
    let error = error
        .map(|message| format!("<p role=\"alert\">{}</p>", escape(message)))
        .unwrap_or_default();
    Ok(Html(format!(
        "<!doctype html><html lang=\"cs\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"robots\" content=\"noindex\"><title>Nastavení odběru · Vyskeř</title><style>body{{font-family:system-ui,sans-serif;line-height:1.6;margin:0;background:#f7f8f5;color:#233a2c}}main{{max-width:42rem;margin:3rem auto;padding:1.5rem;background:white;border-radius:1rem}}fieldset{{border:1px solid #ccd8ce;border-radius:.5rem;margin:1.2rem 0;padding:1rem}}legend{{font-weight:600}}label{{display:block;padding:.45rem 0}}input{{margin-right:.4rem;accent-color:#2c5b3c}}button{{font:inherit;padding:.65rem 1.2rem;background:#2c5b3c;color:white;border:0;border-radius:.4rem;cursor:pointer}}a{{color:#2c5b3c}}[role=status]{{padding:1rem;background:#edf7ed;border-radius:.4rem}}[role=alert]{{padding:1rem;background:#fff0ed;color:#8f241b;border-radius:.4rem}}@media(max-width:46rem){{main{{margin:1rem}}}}</style></head><body><main><h1>Nastavení odběru</h1><p>Vyberte si, které novinky z webu Vyskeř vám mají chodit e-mailem. Přihlášení k účtu není potřeba.</p>{status}{error}<form method=\"post\" action=\"/odber/nastaveni\"><input type=\"hidden\" name=\"token\" value=\"{token}\"><fieldset><legend>Úřední deska</legend>{all}<p>Pro výběr jednotlivých kategorií zrušte volbu „Všechny kategorie“ a zaškrtněte požadované kategorie níže.</p>{choices}{uncategorized}</fieldset><fieldset><legend>Další obsah</legend>{documents}</fieldset><p>Vyberte alespoň jedno téma. Pro ukončení všech zpráv použijte odhlášení odběru.</p><button type=\"submit\">Uložit výběr</button></form><p><a href=\"/odber/odhlasit?token={token}\">Odhlásit celý odběr</a></p><p><a href=\"/\">Zpět na web Vyskeř</a></p></main></body></html>"
    )))
}

// Email scanners can open this link without changing any preferences or tokens.
pub async fn page(
    State(s): State<Backend>,
    Query(input): Query<subscriptions::TokenInput>,
) -> Result<Html<String>> {
    settings_page(&s, &input.token, false, None, None).await
}

pub async fn update(
    State(s): State<Backend>,
    headers: HeaderMap,
    Form(mut fields): Form<HashMap<String, String>>,
) -> Result<Response> {
    // A native form navigation under no-referrer sends Origin: null. Accept
    // that case only with the browser's same-origin navigation metadata. This
    // handler still requires the secret link bound to the current consent.
    let private_form_navigation = headers.get("origin").is_some_and(|value| value == "null")
        && headers
            .get("sec-fetch-site")
            .is_some_and(|value| value == "same-origin")
        && headers
            .get("sec-fetch-mode")
            .is_some_and(|value| value == "navigate");
    if !private_form_navigation {
        auth::check_origin(&headers, &s)?;
    }
    let token = fields
        .remove("token")
        .ok_or_else(|| bad("Odkaz není platný nebo vypršel."))?;
    let mut preferences = SubscriptionPreferences {
        all_notice_categories: false,
        notice_category_ids: Vec::new(),
        uncategorized_notices: false,
        documents: false,
    };
    for (name, value) in fields {
        if value != "on" {
            return Err(bad("Neplatné nastavení odběru."));
        }
        match name.as_str() {
            "all_notice_categories" => preferences.all_notice_categories = true,
            "uncategorized_notices" => preferences.uncategorized_notices = true,
            "documents" => preferences.documents = true,
            _ => {
                let id = name
                    .strip_prefix("category_")
                    .and_then(|id| id.parse::<i64>().ok())
                    .filter(|id| *id > 0)
                    .ok_or_else(|| bad("Neplatné nastavení odběru."))?;
                preferences.notice_category_ids.push(id);
            }
        }
    }
    let now = OffsetDateTime::now_utc();
    let mut tx = crate::db::begin_write(&s.pool).await?;
    let current = current(&mut tx, &token, now).await?;
    let preferences = match subscriptions::validate_preferences(&mut tx, &preferences).await {
        Ok(preferences) => preferences,
        Err(error) => {
            tx.rollback().await?;
            let page = settings_page(&s, &token, false, Some(&preferences), Some(error.1)).await?;
            return Ok((error.0, page).into_response());
        }
    };
    sqlx::query(
        "UPDATE subscribers SET all_notice_categories=$1,notice_category_ids=$2,
         uncategorized_notices=$3,documents=$4 WHERE id=$5",
    )
    .bind(preferences.all_notice_categories)
    .bind(&preferences.notice_category_ids)
    .bind(preferences.uncategorized_notices)
    .bind(preferences.documents)
    .bind(current.id)
    .execute(&mut *tx)
    .await?;
    audit(
        &mut tx,
        None,
        "subscription_preferences_updated",
        "subscriber",
        current.id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(settings_page(&s, &token, true, None, None)
        .await?
        .into_response())
}
