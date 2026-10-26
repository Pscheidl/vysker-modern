//! Public privacy notice and the exact wording used for newsletter consent.
use serde::{Deserialize, Serialize};

pub const CONSENT_VERSION: &str = "newsletter-2026-10-05";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrivacyPolicy {
    pub version: String,
    pub approved_on: Option<String>,
    pub controller_name: String,
    #[serde(default)]
    pub controller_address: String,
    pub controller_email: String,
    #[serde(default)]
    pub dpo_email: String,
    pub processors: String,
    pub international_transfers: String,
    pub public_records_legal_basis: String,
    pub security_legal_basis: String,
    pub consent_evidence_legal_basis: String,
    pub retention: Retention,
    #[serde(default = "default_consent_text")]
    pub consent_text: String,
    #[serde(default = "default_newsletter_terms")]
    pub newsletter_terms: String,
    #[serde(default = "default_rights_text")]
    pub rights_text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PrivacyNotice {
    pub policy: PrivacyPolicy,
    pub consent_version: String,
    pub consent_text: String,
    pub fingerprint: String,
}

impl PrivacyPolicy {
    pub fn consent_text(&self) -> String {
        self.consent_text.clone()
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
            &self.processors,
            &self.international_transfers,
            &self.public_records_legal_basis,
            &self.security_legal_basis,
            &self.consent_evidence_legal_basis,
            &self.consent_text,
            &self.newsletter_terms,
            &self.rights_text,
        ] {
            anyhow::ensure!(
                !value.trim().is_empty()
                    && value.len() <= 10_000
                    && !value
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\t'),
                "Neúplné informace o soukromí"
            );
        }
        anyhow::ensure!(
            self.controller_address.len() <= 10_000,
            "Adresa správce je příliš dlouhá"
        );
        for email in std::iter::once(&self.controller_email)
            .chain((!self.dpo_email.trim().is_empty()).then_some(&self.dpo_email))
        {
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
        ] {
            anyhow::ensure!(
                days >= 1 && days <= 3650,
                "Lhůty uchování musí být 1 až 3650 dní"
            );
        }
        anyhow::ensure!(
            r.backup_days <= 3650,
            "Lhůta záloh musí být 0 až 3650 dní, 0 znamená vypnuté zálohování"
        );
        Ok(())
    }
}

pub fn default_consent_text() -> String {
    "Přihlášením žádáte o zasílání nových dokumentů včetně úřední desky na zadaný e-mail. Odhlásit se můžete kdykoli odkazem v každé zprávě.".into()
}

pub fn default_newsletter_terms() -> String {
    "Na základě dobrovolného souhlasu podle čl. 6 odst. 1 písm. a) GDPR používáme e-mailovou adresu k zasílání nových dokumentů včetně úřední desky. Evidujeme přesné znění souhlasu a čas žádosti, potvrzení a případného odvolání. Bez poskytnutí adresy nelze odběr zajistit, používání ostatních částí webu tím není omezeno.\n\nSouhlas kdykoli odvoláte odkazem v každé novince bez přihlášení. Odvolání nemá vliv na zákonnost předchozího zpracování.\n\nVyhrazujeme si právo tyto podmínky změnit. Aktuální znění je vždy dostupné na této stránce.".into()
}

pub fn default_rights_text() -> String {
    "U správce můžete požádat o přístup k údajům, jejich opravu, výmaz či omezení zpracování. Podle právního důvodu zpracování máte také právo na přenositelnost nebo vznést námitku. Souhlas s odběrem lze odvolat kdykoli.\n\nStížnost můžete podat u [Úřadu pro ochranu osobních údajů](https://uoou.gov.cz/).".into()
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            version: String::new(),
            approved_on: None,
            controller_name: String::new(),
            controller_address: String::new(),
            controller_email: String::new(),
            dpo_email: String::new(),
            processors: String::new(),
            international_transfers: String::new(),
            public_records_legal_basis: String::new(),
            security_legal_basis: String::new(),
            consent_evidence_legal_basis: String::new(),
            consent_text: default_consent_text(),
            newsletter_terms: default_newsletter_terms(),
            rights_text: default_rights_text(),
            retention: Retention {
                pending_days: 2,
                withdrawn_days: 30,
                mail_days: 30,
                audit_days: 365,
                notice_internal_days: 365,
                operational_log_days: 14,
                backup_days: 30,
            },
        }
    }
}
