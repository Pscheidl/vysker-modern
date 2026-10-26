//! Nastavení z proměnných prostředí. Výchozí hodnoty jsou zvolené tak, aby
//! vývojový server fungoval bez jediné proměnné.

use std::net::SocketAddr;

use anyhow::Context;

pub const DEFAULT_MIN_PASSWORD_LENGTH: usize = 24;

/// Connection limits apply to each application process, not to PostgreSQL as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatabasePoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_seconds: u32,
    /// Zero disables closing idle connections.
    pub idle_timeout_seconds: u32,
    /// Zero disables retiring connections based on their age.
    pub max_lifetime_seconds: u32,
}

impl Default for DatabasePoolConfig {
    fn default() -> Self {
        Self::for_environment(false)
    }
}

impl DatabasePoolConfig {
    pub fn for_environment(production: bool) -> Self {
        Self {
            max_connections: if production { 50 } else { 10 },
            min_connections: 0,
            acquire_timeout_seconds: 10,
            idle_timeout_seconds: 600,
            max_lifetime_seconds: 1800,
        }
    }

    fn from_env(production: bool) -> anyhow::Result<Self> {
        Self::from_lookup(production, |name| std::env::var(name).ok())
    }

    // Injecting the lookup keeps configuration tests independent of global environment state.
    fn from_lookup(
        production: bool,
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        let defaults = Self::for_environment(production);
        let mut read = |name: &str, default: u32| -> anyhow::Result<u32> {
            match lookup(name) {
                Some(value) => value.parse().with_context(|| {
                    format!("{name} must be an integer between 0 and {}", u32::MAX)
                }),
                None => Ok(default),
            }
        };
        let config = Self {
            max_connections: read("DB_POOL_MAX_CONNECTIONS", defaults.max_connections)?,
            min_connections: read("DB_POOL_MIN_CONNECTIONS", defaults.min_connections)?,
            acquire_timeout_seconds: read(
                "DB_POOL_ACQUIRE_TIMEOUT_SECONDS",
                defaults.acquire_timeout_seconds,
            )?,
            idle_timeout_seconds: read(
                "DB_POOL_IDLE_TIMEOUT_SECONDS",
                defaults.idle_timeout_seconds,
            )?,
            max_lifetime_seconds: read(
                "DB_POOL_MAX_LIFETIME_SECONDS",
                defaults.max_lifetime_seconds,
            )?,
        };
        config.validate()?;
        Ok(config)
    }

    /// Also checked when opening a pool, including for programmatically built configuration.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_connections > 0
                && (self.max_connections as usize) <= tokio::sync::Semaphore::MAX_PERMITS,
            "DB_POOL_MAX_CONNECTIONS must be positive and within the runtime connection limit"
        );
        anyhow::ensure!(
            self.min_connections <= self.max_connections,
            "DB_POOL_MIN_CONNECTIONS must not exceed DB_POOL_MAX_CONNECTIONS"
        );
        anyhow::ensure!(
            self.acquire_timeout_seconds > 0,
            "DB_POOL_ACQUIRE_TIMEOUT_SECONDS must be positive"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub struct Config {
    /// Adresa, na které server poslouchá. `OBEC_ADRESA`
    pub bind_address: SocketAddr,
    /// Připojovací řetězec PostgreSQL. `OBEC_DATABAZE`
    pub database_url: String,
    pub database_pool: DatabasePoolConfig,
    pub public_url: String,
    pub seed_demo: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: String,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    /// Test-only delivery to the internal capture server, ignoring saved accounts.
    pub smtp_capture_only: bool,
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
        let database_pool = DatabasePoolConfig::from_env(production)?;
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
        let config = Self {
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
            database_pool,
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
            smtp_capture_only: env_var("OBEC_SMTP_CAPTURE_ONLY", "false")
                .parse()
                .context("OBEC_SMTP_CAPTURE_ONLY musí být true nebo false")?,
            mail_settings_key_file: env_var(
                "OBEC_MAIL_SETTINGS_KEY_FILE",
                "data/mail-settings.key",
            )
            .into(),
            email_from: env_var("OBEC_EMAIL_OD", "Vyskeř <noreply@localhost>"),
        };
        config.validate_smtp_capture()?;
        Ok(config)
    }

    /// Also checked at transport creation so a later configuration change fails closed.
    pub fn validate_smtp_capture(&self) -> anyhow::Result<()> {
        if self.smtp_capture_only {
            let local = self
                .smtp_host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback());
            anyhow::ensure!(
                !self.production
                    && ((self.smtp_host == "mailpit" && self.smtp_port == 1025) || local)
                    && self.smtp_tls == "none"
                    && self.smtp_username.is_none()
                    && self.smtp_password.is_none(),
                "OBEC_SMTP_CAPTURE_ONLY vyžaduje neprodukční provoz, mailpit:1025 nebo loopback IP, TLS none a žádné SMTP přihlašovací údaje"
            );
        }
        Ok(())
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

    fn pool_config(
        production: bool,
        values: &[(&str, &str)],
    ) -> anyhow::Result<DatabasePoolConfig> {
        DatabasePoolConfig::from_lookup(production, |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn database_pool_defaults_depend_on_environment() {
        let development = pool_config(false, &[]).unwrap();
        assert_eq!(development.max_connections, 10);
        assert_eq!(development.min_connections, 0);
        assert_eq!(development.acquire_timeout_seconds, 10);
        assert_eq!(development.idle_timeout_seconds, 600);
        assert_eq!(development.max_lifetime_seconds, 1800);
        assert_eq!(
            pool_config(true, &[]).unwrap(),
            DatabasePoolConfig {
                max_connections: 50,
                ..development
            }
        );
    }

    #[test]
    fn database_pool_explicit_values_override_both_environments() {
        for production in [false, true] {
            let config = pool_config(
                production,
                &[
                    ("DB_POOL_MAX_CONNECTIONS", "25"),
                    ("DB_POOL_MIN_CONNECTIONS", "3"),
                    ("DB_POOL_ACQUIRE_TIMEOUT_SECONDS", "8"),
                    ("DB_POOL_IDLE_TIMEOUT_SECONDS", "120"),
                    ("DB_POOL_MAX_LIFETIME_SECONDS", "900"),
                ],
            )
            .unwrap();
            assert_eq!(
                config,
                DatabasePoolConfig {
                    max_connections: 25,
                    min_connections: 3,
                    acquire_timeout_seconds: 8,
                    idle_timeout_seconds: 120,
                    max_lifetime_seconds: 900,
                }
            );
        }
    }

    #[test]
    fn database_pool_allows_zero_only_for_minimum_and_optional_timeouts() {
        let config = pool_config(
            false,
            &[
                ("DB_POOL_MIN_CONNECTIONS", "0"),
                ("DB_POOL_IDLE_TIMEOUT_SECONDS", "0"),
                ("DB_POOL_MAX_LIFETIME_SECONDS", "0"),
            ],
        )
        .unwrap();
        assert_eq!(config.min_connections, 0);
        assert_eq!(config.idle_timeout_seconds, 0);
        assert_eq!(config.max_lifetime_seconds, 0);
        for name in ["DB_POOL_MAX_CONNECTIONS", "DB_POOL_ACQUIRE_TIMEOUT_SECONDS"] {
            assert!(pool_config(false, &[(name, "0")]).is_err());
        }
        assert!(
            pool_config(
                true,
                &[
                    ("DB_POOL_MAX_CONNECTIONS", "2"),
                    ("DB_POOL_MIN_CONNECTIONS", "3"),
                ],
            )
            .is_err()
        );
        assert!(
            pool_config(
                true,
                &[
                    ("DB_POOL_MAX_CONNECTIONS", "2"),
                    ("DB_POOL_MIN_CONNECTIONS", "2"),
                ],
            )
            .is_ok()
        );
    }

    #[test]
    fn database_pool_rejects_malformed_and_overflowing_values() {
        for name in [
            "DB_POOL_MAX_CONNECTIONS",
            "DB_POOL_MIN_CONNECTIONS",
            "DB_POOL_ACQUIRE_TIMEOUT_SECONDS",
            "DB_POOL_IDLE_TIMEOUT_SECONDS",
            "DB_POOL_MAX_LIFETIME_SECONDS",
        ] {
            for value in [
                "",
                "-1",
                "1.5",
                "invalid",
                "4294967296",
                "18446744073709551615",
            ] {
                let error = pool_config(false, &[(name, value)]).unwrap_err();
                assert!(error.to_string().contains(name));
            }
        }
    }

    #[test]
    fn password_policy_defaults_to_twenty_four_and_rejects_invalid_values() {
        assert_eq!(DEFAULT_MIN_PASSWORD_LENGTH, 24);
        assert_eq!(parse_minimum_password_length("32").unwrap(), 32);
        for value in ["", "0", "-1", "1025", "invalid"] {
            assert!(parse_minimum_password_length(value).is_err());
        }
    }
}
