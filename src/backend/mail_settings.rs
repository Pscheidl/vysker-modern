//! Administrator-managed Google SMTP credentials. Secrets never enter API responses.
use super::{Backend, Error, Result, accounts, audit, auth, bad, mail};
use crate::config::Config;
use anyhow::Context;
use axum::{Json, extract::State, http::StatusCode};
use lettre::{AsyncTransport, Message, message::Mailbox};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{fs::OpenOptions, io::Write, path::Path, sync::Arc};
use time::OffsetDateTime;

const GOOGLE_HOST: &str = "smtp.gmail.com";
const GOOGLE_PORT: u16 = 587;
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;

#[derive(sqlx::FromRow)]
struct Saved {
    username: String,
    sender_name: String,
    password_encrypted: Vec<u8>,
}

#[derive(Serialize)]
pub struct Settings {
    provider: &'static str,
    username: String,
    sender_name: String,
    password_configured: bool,
    smtp_host: String,
    smtp_port: u16,
    smtp_tls: String,
    email_from: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    provider: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    sender_name: String,
    password: Option<String>,
    current_password: String,
}

fn configuration_error() -> Error {
    Error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Nastavení e-mailu nelze načíst nebo bezpečně uložit. Požádejte správce serveru o kontrolu šifrovacího klíče.",
    )
}

fn google_sender(username: &str, sender_name: &str) -> anyhow::Result<String> {
    let name = (!sender_name.is_empty()).then(|| sender_name.to_owned());
    Ok(Mailbox::new(name, username.parse()?).to_string())
}

fn view(config: &Config, saved: Option<&Saved>) -> anyhow::Result<Settings> {
    if let Some(saved) = saved {
        Ok(Settings {
            provider: "google",
            username: saved.username.clone(),
            sender_name: saved.sender_name.clone(),
            password_configured: true,
            smtp_host: GOOGLE_HOST.into(),
            smtp_port: GOOGLE_PORT,
            smtp_tls: "starttls".into(),
            email_from: google_sender(&saved.username, &saved.sender_name)?,
        })
    } else {
        Ok(Settings {
            provider: "environment",
            username: config.smtp_username.clone().unwrap_or_default(),
            sender_name: config
                .email_from
                .parse::<Mailbox>()
                .ok()
                .and_then(|address| address.name)
                .unwrap_or_default(),
            password_configured: config.smtp_password.is_some(),
            smtp_host: config.smtp_host.clone(),
            smtp_port: config.smtp_port,
            smtp_tls: config.smtp_tls.clone(),
            email_from: config.email_from.clone(),
        })
    }
}

pub async fn get(State(s): State<Backend>, _: auth::Admin) -> Result<Json<Settings>> {
    let saved: Option<Saved> = sqlx::query_as(
        "SELECT username,sender_name,password_encrypted FROM mail_settings WHERE id=1",
    )
    .fetch_optional(&s.pool)
    .await?;
    Ok(Json(
        view(&s.config, saved.as_ref()).map_err(|_| configuration_error())?,
    ))
}

/// Reloaded by the mail worker so administration changes apply without a restart.
/// Failure to decrypt stops delivery instead of silently selecting another sender.
pub async fn effective_config(s: &Backend) -> anyhow::Result<Config> {
    let saved: Option<Saved> = sqlx::query_as(
        "SELECT username,sender_name,password_encrypted FROM mail_settings WHERE id=1",
    )
    .fetch_optional(&s.pool)
    .await?;
    let mut config = (*s.config).clone();
    if let Some(saved) = saved {
        let key = key_file(&config.mail_settings_key_file, false)?;
        let password = decrypt(&key, &saved.username, &saved.password_encrypted)?;
        config.smtp_host = GOOGLE_HOST.into();
        config.smtp_port = GOOGLE_PORT;
        config.smtp_tls = "starttls".into();
        config.email_from = google_sender(&saved.username, &saved.sender_name)?;
        config.smtp_username = Some(saved.username);
        config.smtp_password = Some(password);
    }
    Ok(config)
}

pub async fn update(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<Update>,
) -> Result<Json<Settings>> {
    let verified = accounts::reauthenticate(&s, &admin, input.current_password).await?;
    if !matches!(input.provider.as_str(), "environment" | "google") {
        return Err(bad("Vyberte nastavení serveru nebo Google."));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    accounts::current(&mut tx, &admin, &verified).await?;
    let existing: Option<Saved> = sqlx::query_as(
        "SELECT username,sender_name,password_encrypted FROM mail_settings WHERE id=1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let saved = if input.provider == "google" {
        let username = auth::email(&input.username)?;
        let sender_name = sender_name(&input.sender_name)?;
        let password = normalize_password(input.password.as_deref())?;
        if password.is_none()
            && existing
                .as_ref()
                .is_none_or(|saved| saved.username != username)
        {
            return Err(bad("Pro tento účet zadejte nové heslo aplikace Google."));
        }
        // Never create a replacement key while an encrypted credential exists.
        let key = key_file(&s.config.mail_settings_key_file, existing.is_none())
            .map_err(|_| configuration_error())?;
        let password_encrypted = if let Some(password) = password {
            encrypt(&key, &username, &password).map_err(|_| configuration_error())?
        } else {
            let existing = existing.as_ref().expect("existing identity checked above");
            // Verify retained ciphertext now so an unusable key cannot look like a save.
            decrypt(&key, &username, &existing.password_encrypted)
                .map_err(|_| configuration_error())?;
            existing.password_encrypted.clone()
        };
        let saved = Saved {
            username,
            sender_name,
            password_encrypted,
        };
        sqlx::query("INSERT INTO mail_settings(id,username,sender_name,password_encrypted,updated_at,updated_by) VALUES (1,$1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET username=excluded.username,sender_name=excluded.sender_name,password_encrypted=excluded.password_encrypted,updated_at=excluded.updated_at,updated_by=excluded.updated_by")
            .bind(&saved.username)
            .bind(&saved.sender_name)
            .bind(&saved.password_encrypted)
            .bind(OffsetDateTime::now_utc().unix_timestamp())
            .bind(admin.id)
            .execute(&mut *tx)
            .await?;
        Some(saved)
    } else {
        sqlx::query("DELETE FROM mail_settings WHERE id=1")
            .execute(&mut *tx)
            .await?;
        None
    };
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "mail_settings",
        1,
        OffsetDateTime::now_utc(),
    )
    .await?;
    let result = view(&s.config, saved.as_ref()).map_err(|_| configuration_error())?;
    tx.commit().await?;
    Ok(Json(result))
}

pub async fn test(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<accounts::Credentials>,
) -> Result<Json<serde_json::Value>> {
    let now = OffsetDateTime::now_utc();
    auth::throttle(&s, &format!("mail-settings-test:{}", admin.id), 3, 900, now).await?;
    let verified = accounts::reauthenticate(&s, &admin, input.current_password).await?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    accounts::current(&mut tx, &admin, &verified).await?;
    audit(
        &mut tx,
        Some(admin.id),
        "test_requested",
        "mail_settings",
        1,
        now,
    )
    .await?;
    tx.commit().await?;

    // SMTP runs outside the write lock and has the transport's bounded timeout.
    let config = effective_config(&s)
        .await
        .map_err(|_| configuration_error())?;
    let sender = config
        .email_from
        .parse()
        .map_err(|_| configuration_error())?;
    let recipient = admin.email.parse().map_err(|_| configuration_error())?;
    let configured = Backend {
        config: Arc::new(config),
        ..s.clone()
    };
    let message = Message::builder()
        .from(sender)
        .to(recipient)
        .subject("Vyskeř: zkouška odesílání e-mailů")
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body("Dobrý den,\n\ntento e-mail ověřuje nastavení odesílání z obecního webu Vyskeř.\n\nObec Vyskeř".to_owned())
        .map_err(|_| configuration_error())?;
    let smtp = mail::transport(&configured).map_err(|_| configuration_error())?;
    if smtp.send(message).await.is_err() {
        // SMTP responses can contain account data, never return or log them.
        tracing::warn!(administrator_id = admin.id, "Zkouška SMTP selhala");
        return Err(Error(
            StatusCode::BAD_GATEWAY,
            "Zkušební e-mail se nepodařilo odeslat. Zkontrolujte účet, heslo aplikace a dostupnost SMTP serveru.",
        ));
    }
    let mut tx = crate::db::begin_write(&s.pool).await?;
    audit(
        &mut tx,
        Some(admin.id),
        "test_sent",
        "mail_settings",
        1,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({"recipient":admin.email})))
}

fn sender_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.chars().count() > 120 || value.chars().any(char::is_control) {
        return Err(bad(
            "Jméno odesílatele smí mít nejvýše 120 znaků a nesmí obsahovat řídicí znaky.",
        ));
    }
    Ok(value.to_owned())
}

fn normalize_password(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value else { return Ok(None) };
    if value.len() > 128 {
        return Err(bad("Heslo aplikace Google musí obsahovat 16 písmen."));
    }
    let password: String = value.chars().filter(|&c| c != ' ').collect();
    if password.is_empty() {
        return Ok(None);
    }
    if password.len() != 16 || !password.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(bad(
            "Heslo aplikace Google musí obsahovat 16 písmen. Použijte heslo aplikace, nikoli běžné heslo účtu.",
        ));
    }
    Ok(Some(password))
}

fn key_file(path: &Path, create: bool) -> anyhow::Result<[u8; KEY_BYTES]> {
    match std::fs::read(path) {
        Ok(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                anyhow::ensure!(
                    std::fs::metadata(path)?.permissions().mode() & 0o077 == 0,
                    "Mail settings key must be readable only by its owner"
                );
            }
            bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid mail settings key length"))
        }
        Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .context("Cannot create mail settings key directory")?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = match options.open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return key_file(path, false);
                }
                Err(error) => return Err(error).context("Cannot create mail settings key"),
            };
            let mut key = [0u8; KEY_BYTES];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| anyhow::anyhow!("Cannot generate mail settings key"))?;
            file.write_all(&key)
                .context("Cannot write mail settings key")?;
            file.sync_all()
                .context("Cannot persist mail settings key")?;
            #[cfg(unix)]
            {
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                std::fs::File::open(parent)?
                    .sync_all()
                    .context("Cannot persist mail settings key directory")?;
            }
            Ok(key)
        }
        Err(error) => Err(error).context("Cannot read existing mail settings key"),
    }
}

fn cipher(key: &[u8; KEY_BYTES]) -> anyhow::Result<LessSafeKey> {
    UnboundKey::new(&aead::CHACHA20_POLY1305, key)
        .map(LessSafeKey::new)
        .map_err(|_| anyhow::anyhow!("Invalid mail settings encryption key"))
}

fn associated_data(username: &str) -> String {
    format!("obecni-web:mail-settings:v1:google:{username}")
}

fn encrypt(key: &[u8; KEY_BYTES], username: &str, password: &str) -> anyhow::Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_BYTES];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| anyhow::anyhow!("Cannot generate mail settings nonce"))?;
    let mut encrypted = password.as_bytes().to_vec();
    cipher(key)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(associated_data(username)),
            &mut encrypted,
        )
        .map_err(|_| anyhow::anyhow!("Cannot encrypt mail settings"))?;
    let mut stored = nonce.to_vec();
    stored.extend(encrypted);
    Ok(stored)
}

fn decrypt(key: &[u8; KEY_BYTES], username: &str, encrypted: &[u8]) -> anyhow::Result<String> {
    anyhow::ensure!(
        encrypted.len() >= NONCE_BYTES + aead::CHACHA20_POLY1305.tag_len(),
        "Invalid encrypted mail settings"
    );
    let nonce: [u8; NONCE_BYTES] = encrypted[..NONCE_BYTES]
        .try_into()
        .expect("nonce length checked");
    let mut ciphertext = encrypted[NONCE_BYTES..].to_vec();
    let plain = cipher(key)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(associated_data(username)),
            &mut ciphertext,
        )
        .map_err(|_| anyhow::anyhow!("Cannot decrypt mail settings"))?;
    String::from_utf8(plain.to_vec()).context("Invalid decrypted mail settings")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_app_password_normalization_and_sender_validation() {
        assert_eq!(
            normalize_password(Some("abcd efgh ijkl mnop"))
                .unwrap()
                .as_deref(),
            Some("abcdefghijklmnop")
        );
        assert_eq!(normalize_password(Some("   ")).unwrap(), None);
        for invalid in [
            "password",
            "abcdefghijklmn01",
            "abcd\nefghijklmnop",
            "ábcdéfghíjklmnop",
        ] {
            assert!(normalize_password(Some(invalid)).is_err());
        }
        assert!(sender_name("Vyskeř\r\nBcc: x@example.com").is_err());
        assert_eq!(sender_name(" Vyskeř ").unwrap(), "Vyskeř");
    }

    #[test]
    fn credentials_are_authenticated_and_bound_to_identity() {
        let key = [7; KEY_BYTES];
        let encrypted = encrypt(&key, "obec@gmail.com", "abcdefghijklmnop").unwrap();
        assert_eq!(
            decrypt(&key, "obec@gmail.com", &encrypted).unwrap(),
            "abcdefghijklmnop"
        );
        assert!(decrypt(&[8; KEY_BYTES], "obec@gmail.com", &encrypted).is_err());
        assert!(decrypt(&key, "jiny@gmail.com", &encrypted).is_err());
        let mut tampered = encrypted;
        tampered[NONCE_BYTES] ^= 1;
        assert!(decrypt(&key, "obec@gmail.com", &tampered).is_err());
        assert!(decrypt(&key, "obec@gmail.com", &[]).is_err());
    }

    #[test]
    fn key_is_persisted_and_never_recreated_for_existing_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mail.key");
        assert!(key_file(&path, false).is_err());
        assert!(!path.exists());
        let key = key_file(&path, true).unwrap();
        assert_eq!(key_file(&path, true).unwrap(), key);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(&path).unwrap();
        assert!(key_file(&path, false).is_err());
        assert!(!path.exists());
    }
}
