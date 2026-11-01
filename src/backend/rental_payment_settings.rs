//! Reusable municipality payment details for rental settlements.
use super::{Backend, Error, Result, audit, auth, bad};
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentSettingsInput {
    pub account_number: String,
    pub bank_code: String,
    pub iban: String,
    pub due_days: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaymentSettings {
    pub account_number: String,
    pub bank_code: String,
    pub iban: String,
    pub due_days: i32,
    pub effective_iban: String,
}

#[derive(Clone, Debug)]
pub struct PaymentDetails {
    pub account_number: String,
    pub bank_code: String,
    pub iban: String,
    pub effective_iban: String,
}

fn check_mod11(value: &str, width: usize, weights: &[u32]) -> bool {
    let digits = format!("{value:0>width$}");
    digits
        .bytes()
        .zip(weights)
        .map(|(c, weight)| u32::from(c - b'0') * weight)
        .sum::<u32>()
        % 11
        == 0
}

fn decimal_remainder(value: &str) -> u32 {
    value.bytes().fold(0, |remainder, digit| {
        (remainder * 10 + u32::from(digit - b'0')) % 97
    })
}

/// Validates a Czech domestic account and computes its IBAN without relying on
/// a bank directory. A bank code's current allocation is not checked here.
pub(super) fn czech_account(account: &str, bank: &str) -> Result<(String, String, String)> {
    let account = account.trim();
    let bank = bank.trim();
    if bank.len() != 4 || !bank.bytes().all(|c| c.is_ascii_digit()) || bank == "0000" {
        return Err(bad("Kód banky musí obsahovat čtyři číslice."));
    }
    let mut parts = account.split('-');
    let first = parts.next().unwrap_or("");
    let (prefix, number) = match parts.next() {
        Some(number) => (first, number),
        None => ("", first),
    };
    if parts.next().is_some()
        || (account.contains('-') && prefix.is_empty())
        || prefix.len() > 6
        || number.is_empty()
        || number.len() > 10
        || !prefix.bytes().all(|c| c.is_ascii_digit())
        || !number.bytes().all(|c| c.is_ascii_digit())
        || number.bytes().all(|c| c == b'0')
        || !check_mod11(prefix, 6, &[10, 5, 8, 4, 2, 1])
        || !check_mod11(number, 10, &[6, 3, 7, 9, 10, 5, 8, 4, 2, 1])
    {
        return Err(bad(
            "Zadejte platné české číslo účtu včetně případného předčíslí.",
        ));
    }
    let prefix_padded = format!("{prefix:0>6}");
    let number_padded = format!("{number:0>10}");
    // The IBAN checksum rearranges CZ00 after the BBAN and expands C and Z.
    let remainder = decimal_remainder(&format!("{bank}{prefix_padded}{number_padded}123500"));
    let effective_iban = format!(
        "CZ{:02}{bank}{prefix_padded}{number_padded}",
        98 - remainder
    );
    let prefix_trimmed = prefix.trim_start_matches('0');
    let number_trimmed = number.trim_start_matches('0');
    let canonical_account = if prefix_trimmed.is_empty() {
        number_trimmed.to_owned()
    } else {
        format!("{prefix_trimmed}-{number_trimmed}")
    };
    Ok((canonical_account, bank.to_owned(), effective_iban))
}

pub fn validate_payment_details(
    account: &str,
    bank: &str,
    optional_iban: &str,
) -> Result<PaymentDetails> {
    let (account_number, bank_code, effective_iban) =
        if account.trim().is_empty() && bank.trim().is_empty() {
            (String::new(), String::new(), String::new())
        } else {
            czech_account(account, bank)?
        };
    let iban = if optional_iban.trim().is_empty() {
        String::new()
    } else {
        let normalized = super::rental_payment::normalized_iban(optional_iban)
            .map_err(|_| bad("Zadejte platný IBAN."))?;
        if normalized != effective_iban {
            return Err(bad(
                "IBAN musí odpovídat zadanému českému účtu a kódu banky.",
            ));
        }
        normalized
    };
    Ok(PaymentDetails {
        account_number,
        bank_code,
        iban,
        effective_iban,
    })
}

fn normalize(input: PaymentSettingsInput) -> Result<PaymentSettings> {
    if !(1..=90).contains(&input.due_days) {
        return Err(bad("Splatnost musí být mezi 1 a 90 dny."));
    }
    let details = validate_payment_details(&input.account_number, &input.bank_code, &input.iban)?;
    Ok(PaymentSettings {
        account_number: details.account_number,
        bank_code: details.bank_code,
        iban: details.iban,
        due_days: input.due_days,
        effective_iban: details.effective_iban,
    })
}

pub async fn read(State(s): State<Backend>, _: auth::Admin) -> Result<Json<PaymentSettings>> {
    let mut tx = s.pool.begin().await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let row: (String, String, String, i32) = sqlx::query_as(
        "SELECT account_number,bank_code,iban,due_days FROM rental_payment_settings WHERE id=1",
    )
    .fetch_one(&mut *tx)
    .await?;
    Ok(Json(normalize(PaymentSettingsInput {
        account_number: row.0,
        bank_code: row.1,
        iban: row.2,
        due_days: row.3,
    })?))
}

pub async fn update(
    State(s): State<Backend>,
    admin: auth::Admin,
    Json(input): Json<PaymentSettingsInput>,
) -> Result<Json<PaymentSettings>> {
    let settings = normalize(input)?;
    let mut tx = crate::db::begin_write(&s.pool).await?;
    super::rental_settings::require_enabled(&mut tx).await?;
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators a JOIN sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND a.active AND a.can_view_identity AND s.token_hash=$2 AND s.expires_at>$3)")
        .bind(admin.id).bind(&admin.token_hash).bind(OffsetDateTime::now_utc().unix_timestamp())
        .fetch_one(&mut *tx).await?;
    if !allowed {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Pro nastavení platebních údajů nemáte oprávnění.",
        ));
    }
    sqlx::query("UPDATE rental_payment_settings SET account_number=$1,bank_code=$2,iban=$3,due_days=$4 WHERE id=1")
        .bind(&settings.account_number).bind(&settings.bank_code).bind(&settings.iban).bind(settings.due_days)
        .execute(&mut *tx).await?;
    audit(
        &mut tx,
        Some(admin.id),
        "updated",
        "rental_payment_settings",
        1,
        OffsetDateTime::now_utc(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(account: &str, bank: &str, iban: &str) -> PaymentSettingsInput {
        PaymentSettingsInput {
            account_number: account.into(),
            bank_code: bank.into(),
            iban: iban.into(),
            due_days: 14,
        }
    }

    #[test]
    fn computes_and_checks_czech_account_and_optional_iban() {
        let settings = normalize(input(
            "000019-2000145399",
            "0800",
            "cz65 0800 0000 1920 0014 5399",
        ))
        .unwrap();
        assert_eq!(settings.account_number, "19-2000145399");
        assert_eq!(settings.bank_code, "0800");
        assert_eq!(settings.iban, "CZ6508000000192000145399");
        assert_eq!(settings.effective_iban, "CZ6508000000192000145399");
        assert_eq!(
            normalize(input("19-2000145399", "0800", ""))
                .unwrap()
                .effective_iban,
            settings.effective_iban
        );
        assert!(normalize(input("19-2000145398", "0800", "")).is_err());
        assert!(normalize(input("18-2000145399", "0800", "")).is_err());
        assert!(normalize(input("0", "0800", "")).is_err());
        assert!(normalize(input("19-2000145399", "800", "")).is_err());
        assert!(normalize(input("19-2000145399", "0100", "CZ6508000000192000145399")).is_err());
        assert!(normalize(input("", "", "CZ6508000000192000145399")).is_err());
        assert!(normalize(input("", "", "")).is_ok());
    }
}
