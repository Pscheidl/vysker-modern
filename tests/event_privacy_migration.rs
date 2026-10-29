#![cfg(feature = "ssr")]

use obecni_web::{backend::auth, privacy::PrivacyPolicy};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;

const OLD_CONSENT: &str = "Přihlášením žádáte o zasílání nových dokumentů včetně úřední desky na zadaný e-mail. Odhlásit se můžete kdykoli odkazem v každé zprávě.";
const OLD_TERMS: &str = "Na základě dobrovolného souhlasu podle čl. 6 odst. 1 písm. a) GDPR používáme e-mailovou adresu k zasílání nových dokumentů včetně úřední desky. Evidujeme přesné znění souhlasu a čas žádosti, potvrzení a případného odvolání. Bez poskytnutí adresy nelze odběr zajistit, používání ostatních částí webu tím není omezeno.\n\nSouhlas kdykoli odvoláte odkazem v každé novince bez přihlášení. Odvolání nemá vliv na zákonnost předchozího zpracování.\n\nVyhrazujeme si právo tyto podmínky změnit. Aktuální znění je vždy dostupné na této stránce.";

#[test]
fn omitted_fallback_wording_includes_events_and_changes_the_notice_fingerprint() {
    let current: PrivacyPolicy =
        serde_json::from_str(include_str!("../config/privacy.example.json")).unwrap();
    assert!(
        current
            .consent_text
            .contains("včetně úřední desky a nově oznámených akcí")
    );
    assert!(
        current
            .newsletter_terms
            .contains("včetně úřední desky a nově oznámených akcí")
    );
    let mut historical = current.clone();
    historical.consent_text = OLD_CONSENT.into();
    historical.newsletter_terms = OLD_TERMS.into();
    assert_ne!(
        current.notice().fingerprint,
        historical.notice().fingerprint
    );
    assert_eq!(
        current.notice().consent_version,
        historical.notice().consent_version
    );
}

#[tokio::test]
async fn calendar_upgrade_updates_only_exact_current_defaults_and_preserves_consent_evidence() {
    let url =
        std::env::var("TEST_DATABASE_URL").expect("Run scripts/test.sh or set TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("event_privacy_upgrade_{}", &auth::token()[..24]);
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('search_path',$1,true)")
        .bind(&schema)
        .execute(&mut *tx)
        .await
        .unwrap();
    let migrations = sqlx::migrate!();
    for migration in migrations
        .iter()
        .filter(|migration| migration.version <= 25)
    {
        sqlx::raw_sql(migration.sql.clone())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let mut original_policy: Value =
        serde_json::from_str(include_str!("../config/privacy.example.json")).unwrap();
    original_policy["version"] = json!("approved-policy-kept");
    original_policy["approved_on"] = json!("2026-10-01");
    original_policy["consent_text"] = json!(OLD_CONSENT);
    original_policy["newsletter_terms"] = json!(OLD_TERMS);
    let original_json = serde_json::to_string_pretty(&original_policy).unwrap();
    sqlx::query("INSERT INTO consent_notices(fingerprint,version,consent_text,privacy_json) VALUES ('old-evidence','original-version',$1,$2)")
        .bind(OLD_CONSENT).bind(&original_json).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql("INSERT INTO administrators(id,email,password_hash) VALUES (7,'privacy-admin@example.test','test-only');
        INSERT INTO subscribers(id,email,verified_at,retention_started_at) VALUES (8,'historical@example.test',110,100);
        INSERT INTO subscription_consents(id,subscriber_id,notice_fingerprint,requested_at,confirmed_at)
            VALUES (9,8,'old-evidence',100,110);")
        .execute(&mut *tx).await.unwrap();
    let consent_before: String =
        sqlx::query_scalar("SELECT to_jsonb(c)::text FROM subscription_consents c WHERE id=9")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let upgrade = migrations
        .iter()
        .find(|migration| migration.version == 26)
        .unwrap();
    let custom_consent = format!("{OLD_CONSENT} Vlastní doplnění.");
    let custom_terms = format!("{OLD_TERMS}\n\nVlastní doplnění.");
    for (label, consent, terms) in [
        ("both defaults", Some(OLD_CONSENT), Some(OLD_TERMS)),
        (
            "both custom",
            Some(custom_consent.as_str()),
            Some(custom_terms.as_str()),
        ),
        (
            "custom consent",
            Some(custom_consent.as_str()),
            Some(OLD_TERMS),
        ),
        (
            "custom terms",
            Some(OLD_CONSENT),
            Some(custom_terms.as_str()),
        ),
        ("omitted fallback keys", None, None),
    ] {
        sqlx::query("SAVEPOINT privacy_upgrade_case")
            .execute(&mut *tx)
            .await
            .unwrap();
        let mut policy = original_policy.clone();
        for (key, value) in [("consent_text", consent), ("newsletter_terms", terms)] {
            if let Some(value) = value {
                policy[key] = json!(value);
            } else {
                policy.as_object_mut().unwrap().remove(key);
            }
        }
        let before = serde_json::to_string_pretty(&policy).unwrap();
        sqlx::query("INSERT INTO privacy_settings(id,policy_json,updated_at,updated_by) VALUES (1,$1,123,7)")
            .bind(&before).execute(&mut *tx).await.unwrap();
        sqlx::raw_sql(upgrade.sql.clone())
            .execute(&mut *tx)
            .await
            .unwrap();
        let (after, updated_at, updated_by): (String, i64, Option<i64>) = sqlx::query_as(
            "SELECT policy_json,updated_at,updated_by FROM privacy_settings WHERE id=1",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let mut expected = policy;
        if consent == Some(OLD_CONSENT) {
            expected["consent_text"] = json!(obecni_web::privacy::default_consent_text());
        }
        if terms == Some(OLD_TERMS) {
            expected["newsletter_terms"] = json!(obecni_web::privacy::default_newsletter_terms());
        }
        assert_eq!(
            serde_json::from_str::<Value>(&after).unwrap(),
            expected,
            "{label}"
        );
        assert_eq!((updated_at, updated_by), (123, Some(7)), "{label}");
        if consent != Some(OLD_CONSENT) && terms != Some(OLD_TERMS) {
            assert_eq!(
                after, before,
                "Untouched custom JSON must remain exact: {label}"
            );
        }
        let evidence: (String, String, String, String) = sqlx::query_as(
            "SELECT fingerprint,version,consent_text,privacy_json FROM consent_notices WHERE fingerprint='old-evidence'")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            evidence,
            (
                "old-evidence".into(),
                "original-version".into(),
                OLD_CONSENT.into(),
                original_json.clone()
            ),
            "{label}"
        );
        let consent_after: String = sqlx::query_scalar(
            "SELECT (to_jsonb(c)-'events')::text FROM subscription_consents c WHERE id=9",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(consent_after, consent_before, "{label}");
        let recorded_events: Option<bool> =
            sqlx::query_scalar("SELECT events FROM subscription_consents WHERE id=9")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(recorded_events, None, "{label}");
        sqlx::query("ROLLBACK TO SAVEPOINT privacy_upgrade_case")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("RELEASE SAVEPOINT privacy_upgrade_case")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.rollback().await.unwrap();
    pool.close().await;
}
