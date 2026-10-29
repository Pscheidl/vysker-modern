//! Bounded reader for the anonymous feed used by ČEZ Distribuce's public widget.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Deserializer};
use std::{collections::BTreeMap, time::Duration};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use time_tz::{OffsetDateTimeExt, timezones::db::europe::PRAGUE};

const API_BASE: &str = "https://api.bezstavy.cz/cezd/api/inspecttown/";
const SOURCE_BASE: &str = "https://www.cezdistribuce.cz/cs/pro-zakazniky/potrebuji-vyresit/stavajici-pripojeni/overeni-planovane-odstavky";
const CDN_BASE: &str = "https://cdn.bezstavy.cz/";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outage {
    pub id: String,
    pub starts_at: OffsetDateTime,
    pub ends_at: OffsetDateTime,
    pub title: String,
    pub description: String,
    pub location: String,
    pub source_url: String,
    pub cancelled: bool,
}

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new(public_url: &str) -> Result<Self> {
        let site = url::Url::parse(public_url).context("Invalid public site URL")?;
        ensure!(
            matches!(site.scheme(), "https" | "http")
                && site.host_str().is_some()
                && site.username().is_empty()
                && site.password().is_none(),
            "Invalid public site URL"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(DEADLINE)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(format!(
                    "ObecniWeb/0.1 (+{})",
                    site.origin().ascii_serialization()
                ))
                .pool_max_idle_per_host(1)
                .build()?,
        })
    }

    pub async fn fetch(&self, town_code: &str) -> Result<Vec<Outage>> {
        validate_town_code(town_code)?;
        tokio::time::timeout(DEADLINE, async {
            let mut response = self
                .http
                .get(format!("{API_BASE}{town_code}"))
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await
                .context("ČEZ outage request failed")?;
            ensure!(
                response.status().is_success(),
                "ČEZ outage HTTP status {}",
                response.status()
            );
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next());
            ensure!(
                content_type
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json")),
                "ČEZ outage response is not JSON"
            );
            ensure!(
                response
                    .content_length()
                    .is_none_or(|length| length <= MAX_RESPONSE as u64),
                "ČEZ outage response exceeds 2 MiB"
            );
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .context("ČEZ outage body read failed")?
            {
                ensure!(
                    bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE,
                    "ČEZ outage response exceeds 2 MiB"
                );
                bytes.extend_from_slice(&chunk);
            }
            parse_snapshot(&bytes, town_code)
        })
        .await
        .context("ČEZ outage request timed out")?
    }
}

// Nullable collections still have to be present in a nonempty snapshot.
fn nullable<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Deserialize)]
struct RawOutage {
    id: String,
    announcement_key: String,
    opened_at: String,
    fix_expected_at: String,
    addresses: Addresses,
    cancelled_at: Option<String>,
}

#[derive(Deserialize)]
struct Addresses {
    towns: Vec<Town>,
    #[serde(deserialize_with = "nullable")]
    orphan_territories: Option<Vec<Territory>>,
}

#[derive(Deserialize)]
struct Town {
    name: String,
    code: u32,
    district: String,
    #[serde(deserialize_with = "nullable")]
    cadastral_territories: Option<Vec<Territory>>,
    town_districts: Vec<TownDistrict>,
}

#[derive(Deserialize)]
struct TownDistrict {
    name: String,
    code: u32,
    town_parts: Vec<TownPart>,
}

#[derive(Deserialize)]
struct TownPart {
    name: String,
    streets: Vec<Street>,
    #[serde(deserialize_with = "nullable")]
    cadastral_territory: Option<Territory>,
}

#[derive(Deserialize)]
struct Street {
    name: String,
    house_nums: String,
    ev_nums: String,
    street_nums: String,
}

#[derive(Deserialize)]
struct Territory {
    name: String,
    code: u32,
    plots: Vec<Plot>,
}

#[derive(Deserialize)]
struct Plot {
    cadastral_code: String,
    plot: String,
}

fn validate_town_code(value: &str) -> Result<u32> {
    ensure!(
        value.len() == 6 && value.bytes().all(|ch| ch.is_ascii_digit()),
        "Invalid ČEZ municipality code"
    );
    let value = value.parse::<u32>()?;
    ensure!(value >= 100_000, "Invalid ČEZ municipality code");
    Ok(value)
}

fn checked_text(value: &str, required: bool) -> Result<&str> {
    ensure!(
        value.len() <= 100_000 && !value.chars().any(char::is_control),
        "Invalid text in ČEZ outage response"
    );
    ensure!(
        !required || !value.trim().is_empty(),
        "Missing text in ČEZ outage response"
    );
    Ok(value.trim())
}

fn markdown(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        match ch {
            '&' => result.push_str("&amp;"),
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '!' => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

fn pdf_url(key: &str) -> Result<String> {
    ensure!(
        key.starts_with("pdf/") && key.ends_with(".pdf"),
        "Invalid ČEZ announcement path"
    );
    let name = &key[4..];
    ensure!(
        !name.is_empty()
            && !name.contains("..")
            && name
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.')),
        "Invalid ČEZ announcement path"
    );
    Ok(format!("{CDN_BASE}{key}"))
}

fn validate_territory(territory: &Territory) -> Result<()> {
    checked_text(&territory.name, true)?;
    ensure!(
        (100_000..=999_999).contains(&territory.code),
        "Invalid cadastral code"
    );
    for plot in &territory.plots {
        ensure!(
            plot.cadastral_code == territory.code.to_string(),
            "Mismatched cadastral code"
        );
        checked_text(&plot.plot, true)?;
    }
    Ok(())
}

fn validate_addresses(addresses: &Addresses) -> Result<()> {
    for town in &addresses.towns {
        checked_text(&town.name, true)?;
        checked_text(&town.district, true)?;
        ensure!(
            (100_000..=999_999).contains(&town.code),
            "Invalid town code"
        );
        for territory in town.cadastral_territories.iter().flatten() {
            validate_territory(territory)?;
        }
        for district in &town.town_districts {
            checked_text(&district.name, false)?;
            ensure!(district.code <= 999_999, "Invalid town district code");
            for part in &district.town_parts {
                checked_text(&part.name, true)?;
                if let Some(territory) = &part.cadastral_territory {
                    validate_territory(territory)?;
                }
                for street in &part.streets {
                    checked_text(&street.name, false)?;
                    checked_text(&street.house_nums, false)?;
                    checked_text(&street.ev_nums, false)?;
                    checked_text(&street.street_nums, false)?;
                    ensure!(
                        !street.house_nums.trim().is_empty()
                            || !street.ev_nums.trim().is_empty()
                            || !street.street_nums.trim().is_empty(),
                        "Street without affected numbers"
                    );
                }
            }
        }
    }
    for territory in addresses.orphan_territories.iter().flatten() {
        validate_territory(territory)?;
    }
    Ok(())
}

fn date_time(value: OffsetDateTime) -> String {
    let local = value.to_timezone(PRAGUE);
    format!(
        "{}. {}. {} {:02}:{:02}",
        local.day(),
        u8::from(local.month()),
        local.year(),
        local.hour(),
        local.minute()
    )
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime> {
    // Keep the upstream contract RFC3339-only, then apply the calendar's
    // checked UTC/Prague conversion before rendering a local timestamp.
    OffsetDateTime::parse(value, &Rfc3339).context("Invalid ČEZ timestamp")?;
    super::events::parse_event_datetime(value)
        .map_err(|error| anyhow::anyhow!("Unsupported ČEZ timestamp: {}", error.1))
}

fn convert(raw: RawOutage, town_code: u32) -> Result<Outage> {
    ensure!(
        !raw.id.is_empty() && raw.id.len() <= 32 && raw.id.bytes().all(|ch| ch.is_ascii_digit()),
        "Invalid ČEZ outage ID"
    );
    let starts_at = parse_timestamp(&raw.opened_at).context("Invalid ČEZ outage start")?;
    let ends_at = parse_timestamp(&raw.fix_expected_at).context("Invalid ČEZ outage end")?;
    ensure!(ends_at > starts_at, "ČEZ outage end is not after start");
    let cancelled = raw
        .cancelled_at
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .context("Invalid ČEZ cancellation timestamp")?
        .is_some();
    let pdf = pdf_url(&raw.announcement_key)?;
    validate_addresses(&raw.addresses)?;
    let towns: Vec<_> = raw
        .addresses
        .towns
        .iter()
        .filter(|town| town.code == town_code)
        .collect();
    ensure!(
        towns.len() == 1,
        "ČEZ outage must contain exactly one requested municipality"
    );
    let town = towns[0];
    let mut address_lines = Vec::new();
    let mut part_names = Vec::new();
    let mut territories: BTreeMap<(u32, String), Vec<String>> = BTreeMap::new();
    let mut add_territory = |territory: &Territory| {
        let plots = territories
            .entry((territory.code, territory.name.trim().to_owned()))
            .or_default();
        plots.extend(
            territory
                .plots
                .iter()
                .map(|plot| plot.plot.trim().to_owned()),
        );
    };
    for territory in town.cadastral_territories.iter().flatten() {
        add_territory(territory);
    }
    for district in &town.town_districts {
        for part in &district.town_parts {
            if let Some(territory) = &part.cadastral_territory {
                add_territory(territory);
            }
            if !part.streets.is_empty() {
                part_names.push(part.name.trim().to_owned());
            }
            for street in &part.streets {
                let mut label = vec![part.name.trim()];
                if !district.name.trim().is_empty() && district.name.trim() != part.name.trim() {
                    label.push(district.name.trim());
                }
                if !street.name.trim().is_empty() && street.name.trim() != part.name.trim() {
                    label.push(street.name.trim());
                }
                let numbers = [
                    ("č. p.", &street.house_nums),
                    ("č. ev.", &street.ev_nums),
                    ("č. o.", &street.street_nums),
                ]
                .into_iter()
                .filter(|(_, value)| !value.trim().is_empty())
                .map(|(kind, value)| format!("{kind} {}", markdown(value.trim())))
                .collect::<Vec<_>>()
                .join(", ");
                address_lines.push(format!("- {}: {numbers}", markdown(&label.join(", "))));
            }
        }
    }
    // Orphan territories have no verified town relationship and must not be
    // silently assigned to the municipality requested by the caller.
    address_lines.sort();
    address_lines.dedup();
    part_names.sort();
    part_names.dedup();
    let mut parcel_lines = Vec::new();
    for ((code, name), mut plots) in territories {
        plots.sort();
        plots.dedup();
        if !plots.is_empty() {
            parcel_lines.push(format!(
                "- Katastrální území {} (kód {code}), parcelní čísla: {}",
                markdown(&name),
                plots
                    .iter()
                    .map(|plot| markdown(plot))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    ensure!(
        !address_lines.is_empty() || !parcel_lines.is_empty(),
        "ČEZ outage has no affected addresses or parcels for the municipality"
    );
    let source_url = format!("{SOURCE_BASE}?jlTown={town_code}");
    let mut description = format!(
        "Plánovaná odstávka elektřiny ČEZ Distribuce č. {}.\n\nTermín: {} až {} (místní čas).",
        raw.id,
        date_time(starts_at),
        date_time(ends_at)
    );
    if cancelled {
        description.push_str("\n\nČEZ Distribuce oznámila zrušení této odstávky.");
    }
    if !address_lines.is_empty() {
        description.push_str("\n\nDotčené adresy v obci:\n\n");
        description.push_str(&address_lines.join("\n"));
    }
    if !parcel_lines.is_empty() {
        description.push_str("\n\nDotčené parcely v obci:\n\n");
        description.push_str(&parcel_lines.join("\n"));
    }
    description.push_str(&format!("\n\n[Původní oznámení ČEZ Distribuce (PDF)]({pdf})\n\n[Aktuální přehled odstávek ČEZ Distribuce]({source_url})"));
    let town_name = town.name.trim();
    let location = if part_names.is_empty() || (part_names.len() == 1 && part_names[0] == town_name)
    {
        town_name.to_owned()
    } else {
        format!("{town_name}: {}", part_names.join(", "))
    };
    Ok(Outage {
        id: raw.id,
        starts_at,
        ends_at,
        title: format!("Odstávka elektřiny: {town_name}"),
        description,
        location,
        source_url,
        cancelled,
    })
}

pub fn parse_snapshot(bytes: &[u8], town_code: &str) -> Result<Vec<Outage>> {
    let town_code = validate_town_code(town_code)?;
    ensure!(
        bytes.len() <= MAX_RESPONSE,
        "ČEZ outage response exceeds 2 MiB"
    );
    let body: serde_json::Value =
        serde_json::from_slice(bytes).context("Invalid ČEZ outage JSON")?;
    let root = body
        .as_object()
        .context("ČEZ outage snapshot is not an object")?;
    ensure!(
        root.get("outages").is_some_and(serde_json::Value::is_null),
        "Unexpected ČEZ municipality snapshot shape"
    );
    // A real no-outage response for Ktová (547484), checked 2026-10-29,
    // contains exactly {"outages":null}, with outages_in_town absent.
    if root.len() == 1 {
        return Ok(Vec::new());
    }
    ensure!(
        root.len() == 2 && root.contains_key("outages_in_town"),
        "Unexpected ČEZ municipality snapshot fields"
    );
    let rows = root["outages_in_town"]
        .as_array()
        .context("ČEZ outages_in_town must be an array")?;
    let mut result = BTreeMap::new();
    for row in rows {
        let raw: RawOutage =
            serde_json::from_value(row.clone()).context("Incomplete ČEZ outage detail")?;
        let outage = convert(raw, town_code)?;
        if let Some(previous) = result.get(&outage.id) {
            if previous != &outage {
                bail!("Conflicting duplicate ČEZ outage ID");
            }
        } else {
            result.insert(outage.id.clone(), outage);
        }
    }
    let mut result: Vec<_> = result.into_values().collect();
    result.sort_by(|a, b| a.starts_at.cmp(&b.starts_at).then(a.id.cmp(&b.id)));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    const REAL: &[u8] = include_bytes!("../../tests/fixtures/cez-vysker.json");
    const EMPTY: &[u8] = include_bytes!("../../tests/fixtures/cez-ktova-empty.json");
    const REAL_PART_LINES: [(&str, &str); 6] = [
        ("Drahoňovice", "- Drahoňovice: č. p. 1-11, č. ev. 1, 2"),
        (
            "Lažany",
            "- Lažany: č. p. 1-5, 7, 8, 9, 11, 12, 13, 14, 16, 19, 20, 25-30, 32, 33, 34, 35, č. ev. 4-9, 11-18",
        ),
        (
            "Mladostov",
            "- Mladostov: č. p. 1-8, 10, 12, 14, 17, 21, č. ev. 1, 2",
        ),
        ("Poddoubí", "- Poddoubí: č. p. 1, 2, 3, 4, 8, 9, 12, 13, 23"),
        ("Skalany", "- Skalany: č. p. 29"),
        (
            "Vyskeř",
            "- Vyskeř: č. p. 1-20, 22-32, 34-62, 64, 65, 66, 67, 69-80, 82-121, 123, 125-134, 138, 139, 142, 144, 145, 147, 151, č. ev. 4, 5, 8",
        ),
    ];

    fn fixture() -> serde_json::Value {
        serde_json::from_slice(REAL).unwrap()
    }

    fn parse(value: &serde_json::Value) -> Result<Vec<Outage>> {
        parse_snapshot(&serde_json::to_vec(value).unwrap(), "577685")
    }

    #[test]
    fn real_vysker_snapshot_preserves_details_and_filters_other_towns() {
        let result = parse_snapshot(REAL, "577685").unwrap();
        assert_eq!(result.len(), 1);
        let event = &result[0];
        assert_eq!(event.id, "110061127971");
        assert_eq!(
            event.starts_at,
            time::macros::datetime!(2026-11-18 06:30 UTC)
        );
        assert_eq!(event.ends_at, time::macros::datetime!(2026-11-18 14:30 UTC));
        assert!(!event.cancelled);
        for expected in [
            "18. 11. 2026 07:30",
            "18. 11. 2026 15:30",
            "https://cdn.bezstavy.cz/pdf/301320195-dbh40tlct0gc12peu50g.pdf",
        ] {
            assert!(event.description.contains(expected), "Missing {expected}");
        }
        for (part, expected_line) in REAL_PART_LINES {
            assert!(
                event.description.lines().any(|line| line == expected_line),
                "Missing complete address list for {part}"
            );
        }
        let parcels = event
            .description
            .lines()
            .find_map(|line| {
                line.strip_prefix("- Katastrální území Vyskeř (kód 787744), parcelní čísla: ")
            })
            .expect("The real outage must include its Vyskeř cadastral parcel list");
        let actual: std::collections::BTreeSet<_> = parcels.split(", ").collect();
        let expected = std::collections::BTreeSet::from([
            "12", "18/2", "63", "72/5", "72/7", "86/3", "110/22", "110/25", "110/27", "110/31",
            "115", "217/2", "286", "289", "371", "418", "423", "614/2", "936/1", "1977/9", "3708",
            "3778", "3949",
        ]);
        assert_eq!(actual.len(), 23);
        assert_eq!(actual, expected);
        for unrelated in ["Hrubá Skála", "Kacanovy", "Libošovice", "Telčice"] {
            assert!(!event.description.contains(unrelated));
        }
        assert!(event.source_url.ends_with("?jlTown=577685"));
    }

    #[test]
    fn imports_an_outage_affecting_only_any_one_of_the_six_municipality_parts() {
        for (selected_name, expected_line) in REAL_PART_LINES {
            let mut value = fixture();
            let addresses = &mut value["outages_in_town"][0]["addresses"];
            addresses["orphan_territories"] = serde_json::Value::Null;
            let town = addresses["towns"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|town| town["code"] == 577685)
                .unwrap();
            town["cadastral_territories"] = serde_json::Value::Null;
            let parts = town["town_districts"][0]["town_parts"]
                .as_array_mut()
                .unwrap();
            parts.retain(|part| part["name"] == selected_name);
            assert_eq!(parts.len(), 1);

            let result = parse(&value).unwrap();
            assert_eq!(result.len(), 1, "Missing outage for {selected_name}");
            let event = &result[0];
            assert!(
                event.description.lines().any(|line| line == expected_line),
                "Missing affected house/evidence numbers for {selected_name}"
            );
            assert!(event.location.contains(selected_name));
            assert!(!event.description.contains("Dotčené parcely"));
            for (other_name, _) in REAL_PART_LINES {
                if other_name != selected_name {
                    let prefix = format!("- {other_name}:");
                    assert!(
                        !event
                            .description
                            .lines()
                            .any(|line| line.starts_with(&prefix)),
                        "Included unaffected part {other_name} for {selected_name}"
                    );
                }
            }
        }
    }

    #[test]
    fn accepts_only_verified_empty_shape_and_explicit_empty_array() {
        assert!(parse_snapshot(EMPTY, "547484").unwrap().is_empty());
        assert!(
            parse_snapshot(br#"{"outages":null,"outages_in_town":[]}"#, "577685")
                .unwrap()
                .is_empty()
        );
        for invalid in [
            "{}",
            "null",
            "[]",
            r#"{"outages_in_town":[]}"#,
            r#"{"outages":null,"outages_in_town":null}"#,
            r#"{"outages":null,"error":"unavailable"}"#,
            r#"{"outages":[]}"#,
        ] {
            assert!(
                parse_snapshot(invalid.as_bytes(), "577685").is_err(),
                "Accepted {invalid}"
            );
        }
    }

    #[test]
    fn rejects_partial_snapshot_even_after_valid_entry() {
        let mut value = fixture();
        value["outages_in_town"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id":"999"}));
        assert!(parse(&value).is_err());
        let mut value = fixture();
        value["outages_in_town"][0]["addresses"]["towns"][3]
            .as_object_mut()
            .unwrap()
            .remove("town_districts");
        assert!(parse(&value).is_err());
    }

    #[test]
    fn deduplicates_identical_ids_but_rejects_conflicting_duplicates() {
        let mut value = fixture();
        let duplicate = value["outages_in_town"][0].clone();
        value["outages_in_town"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert_eq!(parse(&value).unwrap().len(), 1);
        value["outages_in_town"][1]["fix_expected_at"] = "2026-11-18T15:30:00Z".into();
        assert!(parse(&value).is_err());
    }

    #[test]
    fn rejects_invalid_time_municipality_and_pdf_path() {
        for (field, bad) in [
            ("opened_at", "tomorrow"),
            ("fix_expected_at", "2026-11-18T05:00:00Z"),
            ("announcement_key", "pdf/../elsewhere.pdf"),
            ("announcement_key", "https://example.com/file.pdf"),
        ] {
            let mut value = fixture();
            value["outages_in_town"][0][field] = bad.into();
            assert!(parse(&value).is_err(), "Accepted {field}={bad}");
        }
        assert!(parse_snapshot(REAL, "../577685").is_err());
        assert!(parse_snapshot(REAL, "577677").is_err());
    }

    #[test]
    fn cancellation_schema_test_uses_synthetic_field_supported_by_official_widget() {
        let mut value = fixture();
        value["outages_in_town"][0]["cancelled_at"] = "2026-10-29T10:00:00Z".into();
        assert!(parse(&value).unwrap()[0].cancelled);
        value["outages_in_town"][0]["cancelled_at"] = serde_json::Value::Null;
        assert!(!parse(&value).unwrap()[0].cancelled);
        for bad in ["", "not-a-date"] {
            value["outages_in_town"][0]["cancelled_at"] = bad.into();
            assert!(parse(&value).is_err());
        }
    }

    #[test]
    fn renders_orientation_numbers_and_nested_parcels_without_injecting_markdown() {
        let mut value = fixture();
        let town = &mut value["outages_in_town"][0]["addresses"]["towns"][3];
        let part = &mut town["town_districts"][0]["town_parts"][0];
        part["streets"][0]["street_nums"] = "1, 3".into();
        part["streets"][0]["name"] = "<script>[link](evil)".into();
        part["cadastral_territory"] = serde_json::json!({"name":"Vyskeř","code":787744,"plots":[{"cadastral_code":"787744","plot":"ST.123/1"}]});
        let event = &parse(&value).unwrap()[0];
        assert!(event.description.contains("č. o. 1, 3"));
        assert!(event.description.contains("ST.123/1"));
        assert!(event.description.contains(r"\<script\>\[link\](evil)"));
    }

    #[test]
    fn response_size_is_bounded() {
        assert!(parse_snapshot(&vec![b' '; MAX_RESPONSE + 1], "577685").is_err());
    }

    #[test]
    fn extreme_timestamps_fail_without_panicking() {
        for field in ["opened_at", "fix_expected_at", "cancelled_at"] {
            for timestamp in [
                "9999-12-31T23:30:00Z",
                "0000-01-01T00:00:00Z",
                "0001-01-01T00:00:00+14:00",
                "2026-11-18T07:30:00",
            ] {
                let mut value = fixture();
                value["outages_in_town"][0][field] = timestamp.into();
                let result = std::panic::catch_unwind(|| parse(&value));
                assert!(result.is_ok(), "Panicked for {field}={timestamp}");
                assert!(result.unwrap().is_err(), "Accepted {field}={timestamp}");
            }
        }
    }

    #[tokio::test]
    #[ignore = "Requires live public ČEZ API access"]
    async fn live_public_api_snapshot_parses() {
        Client::new("https://www.vysker.cz")
            .unwrap()
            .fetch("577685")
            .await
            .expect("The live public ČEZ municipality snapshot should parse");
    }
}
