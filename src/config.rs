//! Nastavení z proměnných prostředí. Výchozí hodnoty jsou zvolené tak, aby
//! vývojový server fungoval bez jediné proměnné.

use std::net::SocketAddr;

use anyhow::Context;

pub const DEFAULT_MIN_PASSWORD_LENGTH: usize = 24;

#[derive(Clone)]
pub struct Config {
    /// Adresa, na které server poslouchá. `OBEC_ADRESA`
    pub bind_address: SocketAddr,
    /// Připojovací řetězec PostgreSQL. `OBEC_DATABAZE`
    pub database_url: String,
    pub public_url: String,
    pub seed_demo: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: String,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub mail_settings_key_file: std::path::PathBuf,
    pub email_from: String,
    pub production: bool,
    pub minimum_password_length: usize,
    pub trusted_proxy: Option<std::net::IpAddr>,
    pub privacy: Option<crate::privacy::PrivacyPolicy>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_address = env_var("OBEC_ADRESA", "127.0.0.1:3000");
        let public_url = env_var("OBEC_VEREJNA_URL", "http://localhost:3000");
        let url = url::Url::parse(&public_url).context("OBEC_VEREJNA_URL není platná URL")?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none(),
            "OBEC_VEREJNA_URL musí být http(s) adresa bez cesty, přihlašovacích údajů a parametrů"
        );
        let smtp_tls = env_var("OBEC_SMTP_TLS", "starttls");
        anyhow::ensure!(
            matches!(smtp_tls.as_str(), "starttls" | "tls" | "none"),
            "OBEC_SMTP_TLS musí být starttls, tls nebo none"
        );
        let smtp_username = std::env::var("OBEC_SMTP_UZIVATEL").ok();
        let smtp_password = match (
            std::env::var("OBEC_SMTP_HESLO").ok(),
            std::env::var("OBEC_SMTP_HESLO_FILE").ok(),
        ) {
            (Some(_), Some(_)) => anyhow::bail!("Nastavte SMTP heslo buď přímo, nebo ze souboru"),
            (value, None) => value,
            (None, Some(path)) => Some(
                std::fs::read_to_string(path)?
                    .trim_end_matches(['\r', '\n'])
                    .into(),
            ),
        };
        anyhow::ensure!(
            smtp_username.is_some() == smtp_password.is_some(),
            "SMTP uživatel a heslo musí být nastavené společně"
        );
        anyhow::ensure!(
            smtp_tls != "none" || smtp_username.is_none(),
            "SMTP heslo nelze poslat bez TLS"
        );
        let production: bool = env_var("OBEC_PRODUCTION", "false")
            .parse()
            .context("OBEC_PRODUCTION musí být true nebo false")?;
        let privacy = std::env::var("OBEC_PRIVACY_CONFIG")
            .ok()
            .map(|path| -> anyhow::Result<crate::privacy::PrivacyPolicy> {
                let policy: crate::privacy::PrivacyPolicy =
                    serde_json::from_slice(&std::fs::read(path)?)?;
                policy.validate()?;
                Ok(policy)
            })
            .transpose()?;
        if production {
            anyhow::ensure!(
                url.scheme() == "https" && smtp_tls != "none",
                "Produkce vyžaduje HTTPS a šifrované SMTP"
            );
            let policy = privacy
                .as_ref()
                .context("Produkce vyžaduje OBEC_PRIVACY_CONFIG")?;
            anyhow::ensure!(
                policy.approved_on.is_some(),
                "Produkce vyžaduje schválené informace o soukromí"
            );
            let serialized = serde_json::to_string(policy)?;
            anyhow::ensure!(
                !serialized.contains("DOPLNIT") && !serialized.contains("example.test"),
                "Nahraďte ukázkové údaje v nastavení soukromí"
            );
            anyhow::ensure!(
                env_var("OBEC_UKAZKOVA_DATA", "false") == "false",
                "Produkce nesmí vkládat ukázkové údaje"
            );
        }
        let database_url = match (
            std::env::var("OBEC_DATABAZE").ok(),
            std::env::var("OBEC_DATABAZE_FILE").ok(),
        ) {
            (Some(_), Some(_)) => {
                anyhow::bail!("Set OBEC_DATABAZE or OBEC_DATABAZE_FILE, not both")
            }
            (Some(value), None) => value,
            (None, Some(path)) => std::fs::read_to_string(path)?.trim().to_owned(),
            (None, None) => "postgresql://vysker:vysker@127.0.0.1:5432/vysker".into(),
        };
        anyhow::ensure!(
            database_url.starts_with("postgresql://") || database_url.starts_with("postgres://"),
            "OBEC_DATABAZE must be a PostgreSQL URL"
        );
        let minimum_password_length = parse_minimum_password_length(&env_var(
            "OBEC_MIN_PASSWORD_LENGTH",
            &DEFAULT_MIN_PASSWORD_LENGTH.to_string(),
        ))?;
        Ok(Self {
            minimum_password_length,
            trusted_proxy: std::env::var("OBEC_TRUSTED_PROXY")
                .ok()
                .map(|v| v.parse())
                .transpose()
                .context("Neplatná adresa důvěryhodné proxy")?,
            production,
            privacy,
            bind_address: bind_address
                .parse()
                .with_context(|| format!("OBEC_ADRESA není platná adresa: {bind_address}"))?,
            database_url,
            public_url: url.origin().ascii_serialization(),
            seed_demo: env_var("OBEC_UKAZKOVA_DATA", "false")
                .parse()
                .context("OBEC_UKAZKOVA_DATA musí být true nebo false")?,
            smtp_host: env_var("OBEC_SMTP_HOST", "localhost"),
            smtp_port: env_var("OBEC_SMTP_PORT", "587")
                .parse()
                .context("OBEC_SMTP_PORT není číslo portu")?,
            smtp_tls,
            smtp_username,
            smtp_password,
            mail_settings_key_file: env_var(
                "OBEC_MAIL_SETTINGS_KEY_FILE",
                "data/mail-settings.key",
            )
            .into(),
            email_from: env_var("OBEC_EMAIL_OD", "Vyskeř <noreply@localhost>"),
        })
    }
}

fn env_var(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn parse_minimum_password_length(value: &str) -> anyhow::Result<usize> {
    let value: usize = value
        .parse()
        .context("OBEC_MIN_PASSWORD_LENGTH must be an integer")?;
    anyhow::ensure!(
        (1..=1024).contains(&value),
        "OBEC_MIN_PASSWORD_LENGTH must be between 1 and 1024 characters"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_policy_defaults_to_twenty_four_and_rejects_invalid_values() {
        assert_eq!(DEFAULT_MIN_PASSWORD_LENGTH, 24);
        assert_eq!(parse_minimum_password_length("32").unwrap(), 32);
        for value in ["", "0", "-1", "1025", "invalid"] {
            assert!(parse_minimum_password_length(value).is_err());
        }
    }
}
