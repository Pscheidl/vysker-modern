//! Public privacy notice and the exact wording used for newsletter consent.
use serde::{Deserialize, Serialize};

pub const CONSENT_VERSION: &str = "newsletter-2026-09-29.2";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Retention {
    pub pending_days: u16,
    pub withdrawn_days: u16,
    pub mail_days: u16,
    pub audit_days: u16,
    pub notice_internal_days: u16,
    pub operational_log_days: u16,
    pub backup_days: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyPolicy {
    pub version: String,
    pub approved_on: Option<String>,
    pub controller_name: String,
    pub controller_address: String,
    pub controller_email: String,
    pub dpo_email: String,
    pub processors: String,
    pub international_transfers: String,
    pub public_records_legal_basis: String,
    pub security_legal_basis: String,
    pub consent_evidence_legal_basis: String,
    pub retention: Retention,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PrivacyNotice {
    pub policy: PrivacyPolicy,
    pub consent_version: String,
    pub consent_text: String,
    pub fingerprint: String,
}

impl PrivacyPolicy {
    pub fn consent_text(&self) -> String {
        format!(
            "Přihlášením žádáte správce {} o zasílání nových dokumentů včetně úřední desky na zadaný e-mail. Odhlásit se můžete kdykoli odkazem v každé zprávě.",
            self.controller_name
        )
    }

    #[cfg(feature = "ssr")]
    pub fn notice(&self) -> PrivacyNotice {
        let consent_text = self.consent_text();
        let fingerprint = crate::backend::auth::hash(
            serde_json::to_vec(&(CONSENT_VERSION, &consent_text, self))
                .expect("serializable policy"),
        );
        PrivacyNotice {
            policy: self.clone(),
            consent_version: CONSENT_VERSION.into(),
            consent_text,
            fingerprint,
        }
    }

    #[cfg(feature = "ssr")]
    pub fn validate(&self) -> anyhow::Result<()> {
        for value in [
            &self.version,
            &self.controller_name,
            &self.controller_address,
            &self.processors,
            &self.international_transfers,
            &self.public_records_legal_basis,
            &self.security_legal_basis,
            &self.consent_evidence_legal_basis,
        ] {
            anyhow::ensure!(
                !value.trim().is_empty() && value.len() <= 10_000,
                "Neúplné informace o soukromí"
            );
        }
        for email in [&self.controller_email, &self.dpo_email] {
            anyhow::ensure!(
                crate::backend::auth::email(email).is_ok(),
                "Neplatný kontakt správce nebo pověřence"
            );
        }
        if let Some(date) = &self.approved_on {
            let date = time::Date::parse(
                date,
                &time::format_description::well_known::Iso8601::DEFAULT,
            )?;
            anyhow::ensure!(
                date <= time::OffsetDateTime::now_utc().date(),
                "Schválení soukromí nesmí být v budoucnosti"
            );
        }
        let r = &self.retention;
        anyhow::ensure!(
            r.pending_days >= 1 && r.pending_days <= 30,
            "Lhůta nepotvrzených odběrů musí být 1 až 30 dní"
        );
        for days in [
            r.withdrawn_days,
            r.mail_days,
            r.audit_days,
            r.notice_internal_days,
            r.operational_log_days,
            r.backup_days,
        ] {
            anyhow::ensure!(
                days >= 1 && days <= 3650,
                "Lhůty uchování musí být 1 až 3650 dní"
            );
        }
        Ok(())
    }
}
