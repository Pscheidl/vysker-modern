//! Small, bounded Photon proxy for user-triggered address suggestions.
use super::{Backend, Error, Result, auth, bad};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::LazyLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const ENDPOINT: &str = "https://photon.komoot.io/api/";
const CACHE_SIZE: usize = 256;
const CACHE_TTL: Duration = Duration::from_secs(3600);
const MAX_RESPONSE: usize = 128 * 1024;
static SEARCH: LazyLock<Option<AddressSearch>> =
    LazyLock::new(|| AddressSearch::new(ENDPOINT).ok());

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub municipality: Option<String>,
    #[serde(default)]
    pub locality: Option<String>,
    /// Advisory address metadata, not evidence of the applicant's residence.
    #[serde(default)]
    pub is_vysker: bool,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

struct CacheEntry {
    query: String,
    created: Instant,
    suggestions: Vec<Suggestion>,
}

struct SearchState {
    cache: VecDeque<CacheEntry>,
    last_request: Option<Instant>,
}

struct AddressSearch {
    endpoint: String,
    client: reqwest::Client,
    state: Mutex<SearchState>,
}

fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Našeptávání adres nyní není dostupné. Adresu můžete vyplnit ručně nebo označit místo na mapě.",
    )
}

fn normalize_query(query: &str) -> Result<String> {
    if query.chars().count() > 200 || query.chars().any(char::is_control) {
        return Err(bad(
            "Adresa je příliš dlouhá nebo obsahuje nepovolené znaky.",
        ));
    }
    Ok(query.split_whitespace().collect::<Vec<_>>().join(" "))
}

impl AddressSearch {
    fn new(endpoint: &str) -> std::result::Result<Self, reqwest::Error> {
        Ok(Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(4))
                .redirect(reqwest::redirect::Policy::none())
                .pool_max_idle_per_host(1)
                .build()?,
            state: Mutex::new(SearchState {
                cache: VecDeque::new(),
                last_request: None,
            }),
        })
    }

    async fn lookup(&self, query: &str, site: &str) -> Result<Vec<Suggestion>> {
        // One upstream request at a time, at most one per second across visitors.
        // The outer deadline also bounds time spent waiting for this lock.
        let mut state = self.state.lock().await;
        state
            .cache
            .retain(|entry| entry.created.elapsed() < CACHE_TTL);
        let key = query.to_lowercase();
        if let Some(entry) = state.cache.iter().find(|entry| entry.query == key) {
            return Ok(entry.suggestions.clone());
        }
        if let Some(last) = state.last_request {
            tokio::time::sleep(Duration::from_secs(1).saturating_sub(last.elapsed())).await;
        }
        state.last_request = Some(Instant::now());
        let mut response = self
            .client
            .get(&self.endpoint)
            .header(
                reqwest::header::USER_AGENT,
                format!("VyskerObecniWeb/0.1 (+{site})"),
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[
                ("q", query),
                ("limit", "6"),
                ("countrycode", "CZ"),
                ("lat", "50.5295088"),
                ("lon", "15.158555"),
                ("zoom", "14"),
                ("location_bias_scale", "0.1"),
            ])
            .send()
            .await
            .map_err(|_| unavailable())?
            .error_for_status()
            .map_err(|_| unavailable())?;
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE as u64)
        {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        let suggestions = suggestions(&body)?;
        if state.cache.len() == CACHE_SIZE {
            state.cache.pop_front();
        }
        state.cache.push_back(CacheEntry {
            query: key,
            created: Instant::now(),
            suggestions: suggestions.clone(),
        });
        Ok(suggestions)
    }
}

fn property_text<'a>(properties: &'a serde_json::Value, key: &str) -> &'a str {
    properties
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::trim)
        .filter(|value| value.chars().count() <= 300)
        .unwrap_or("")
}

fn vysker_part(name: &str) -> Option<&'static str> {
    // Official municipality 577685 and its six parts:
    // https://risy.gov.cz/cs/vyhledavace/uzemi/577685-vysker
    match name.to_lowercase().as_str() {
        "vyskeř" | "vysker" => Some("Vyskeř"),
        "drahoňovice" | "drahonovice" => Some("Drahoňovice"),
        "lažany" | "lazany" => Some("Lažany"),
        "mladostov" => Some("Mladostov"),
        "poddoubí" | "poddoubi" => Some("Poddoubí"),
        "skalany" => Some("Skalany"),
        _ => None,
    }
}

fn municipality_details(
    properties: &serde_json::Value,
    latitude: f64,
    longitude: f64,
) -> (Option<String>, Option<String>, bool) {
    let text = |key| property_text(properties, key);
    let explicit_municipality = text("municipality");
    let city = text("city");
    let parent = if explicit_municipality.is_empty() {
        city
    } else {
        explicit_municipality
    };
    let locality = [text("district"), text("locality")]
        .into_iter()
        .find(|value| !value.is_empty())
        .or_else(|| (!city.is_empty() && city != parent).then_some(city));
    let known_part = locality.and_then(vysker_part).or_else(|| vysker_part(city));
    // Some geocoders put the village in `city` and omit its parent. A part
    // name alone is ambiguous, as are a postcode or proximity alone. Require
    // all three independent address fields and a broad geographic sanity
    // guard. This rectangle is deliberately not treated as a municipal border.
    let part_without_parent = explicit_municipality.is_empty()
        && (city.is_empty() || vysker_part(city).is_some())
        && known_part.is_some()
        && text("postcode")
            .chars()
            .filter(|value| !value.is_whitespace())
            .eq("51264".chars())
        && matches!(
            text("county").to_lowercase().as_str(),
            "semily" | "okres semily"
        )
        && (50.48..=50.58).contains(&latitude)
        && (15.10..=15.22).contains(&longitude);
    let is_vysker = text("countrycode") == "CZ"
        && (vysker_part(parent) == Some("Vyskeř") || part_without_parent);
    let municipality = if is_vysker {
        Some("Vyskeř".to_owned())
    } else {
        (!parent.is_empty()).then(|| parent.to_owned())
    };
    let locality = if is_vysker {
        known_part.map(str::to_owned)
    } else {
        locality.map(str::to_owned)
    };
    (municipality, locality, is_vysker)
}

fn suggestions(body: &serde_json::Value) -> Result<Vec<Suggestion>> {
    let features = body
        .get("features")
        .and_then(|v| v.as_array())
        .ok_or_else(unavailable)?;
    let mut result: Vec<Suggestion> = Vec::new();
    for feature in features.iter().take(20) {
        let Some(properties) = feature.get("properties") else {
            continue;
        };
        let text = |key| property_text(properties, key);
        if text("countrycode") != "CZ" || feature["geometry"]["type"] != "Point" {
            continue;
        }
        let Some(coordinates) = feature["geometry"]["coordinates"].as_array() else {
            continue;
        };
        let (Some(longitude), Some(latitude)) = (
            coordinates.first().and_then(|v| v.as_f64()),
            coordinates.get(1).and_then(|v| v.as_f64()),
        ) else {
            continue;
        };
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            continue;
        }
        let city = text("city");
        let street = if text("street").is_empty() {
            city
        } else {
            text("street")
        };
        let address = [street, text("housenumber")]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let locality = [text("postcode"), city]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let mut parts = Vec::new();
        for part in [text("name"), &address, text("district")] {
            if !part.is_empty() && !parts.contains(&part) && (part != city || locality.is_empty()) {
                parts.push(part);
            }
        }
        if !locality.is_empty() && !parts.contains(&locality.as_str()) {
            parts.push(&locality);
        }
        let label = parts.join(", ");
        if label.is_empty() || label.chars().count() > 300 {
            continue;
        }
        let (municipality, locality, is_vysker) =
            municipality_details(properties, latitude, longitude);
        let item = Suggestion {
            label,
            latitude,
            longitude,
            municipality,
            locality,
            is_vysker,
        };
        if !result.contains(&item) {
            result.push(item);
        }
        if result.len() == 6 {
            break;
        }
    }
    Ok(result)
}

pub async fn search(
    State(state): State<Backend>,
    auth::ClientIp(ip): auth::ClientIp,
    Query(input): Query<SearchQuery>,
) -> Result<Json<Vec<Suggestion>>> {
    let query = normalize_query(&input.q)?;
    if query.chars().count() < 3 {
        return Ok(Json(Vec::new()));
    }
    auth::throttle(
        &state,
        &format!("report-address:{ip}"),
        60,
        60,
        time::OffsetDateTime::now_utc(),
    )
    .await?;
    let service = SEARCH.as_ref().ok_or_else(unavailable)?;
    let result = tokio::time::timeout(
        Duration::from_secs(6),
        service.lookup(&query, &state.config.public_url),
    )
    .await
    .map_err(|_| unavailable())??;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_queries_and_formats_czech_addresses() {
        assert_eq!(normalize_query("  Vyskeř   50 ").unwrap(), "Vyskeř 50");
        assert!(normalize_query(&"a".repeat(201)).is_err());
        assert!(normalize_query("Vyskeř\n50").is_err());
        let valid = json!({"properties":{"street":"Vyskeř","housenumber":"50","city":"Vyskeř","postcode":"512 64","countrycode":"CZ"},"geometry":{"type":"Point","coordinates":[15.158555,50.5295088]}});
        let invalid = json!({"properties":{"name":"Bad","countrycode":"CZ"},"geometry":{"type":"Point","coordinates":[15,100]}});
        let found = suggestions(&json!({"features":[valid.clone(),valid,invalid]})).unwrap();
        assert_eq!(
            found,
            vec![Suggestion {
                label: "Vyskeř 50, 512 64 Vyskeř".into(),
                latitude: 50.5295088,
                longitude: 15.158555,
                municipality: Some("Vyskeř".into()),
                locality: Some("Vyskeř".into()),
                is_vysker: true,
            }]
        );
        assert!(suggestions(&json!({"error":"unavailable"})).is_err());
        let town = json!({"properties":{"name":"Vyskeř","city":"Vyskeř","countrycode":"CZ"},"geometry":{"type":"Point","coordinates":[15.158555,50.5295088]}});
        assert_eq!(
            suggestions(&json!({"features":[town]})).unwrap()[0].label,
            "Vyskeř"
        );
    }

    fn address_suggestion(mut properties: serde_json::Value, coordinates: [f64; 2]) -> Suggestion {
        properties["countrycode"] = json!("CZ");
        properties["name"] = json!("Vybraná adresa");
        suggestions(&json!({"features":[{
            "properties":properties,
            "geometry":{"type":"Point","coordinates":coordinates}
        }]}))
        .unwrap()
        .remove(0)
    }

    #[test]
    fn identifies_vysker_and_its_parts_from_structured_municipality() {
        let coordinates = [15.158555, 50.5295088];
        for part in [
            "Vyskeř",
            "Drahoňovice",
            "Lažany",
            "Mladostov",
            "Poddoubí",
            "Skalany",
        ] {
            let suggestion =
                address_suggestion(json!({"city":"Vyskeř","district":part}), coordinates);
            assert!(suggestion.is_vysker, "{part}");
            assert_eq!(suggestion.municipality.as_deref(), Some("Vyskeř"));
            assert_eq!(suggestion.locality.as_deref(), Some(part));
        }
        let explicit = address_suggestion(
            json!({"municipality":"Vyskeř","city":"Skalany"}),
            coordinates,
        );
        assert!(explicit.is_vysker);
        assert_eq!(explicit.locality.as_deref(), Some("Skalany"));
        assert!(address_suggestion(json!({"city":" VYSKER "}), coordinates).is_vysker);
    }

    #[test]
    fn requires_postcode_county_and_location_for_a_part_without_its_parent() {
        let coordinates = [15.158555, 50.5295088];
        for part in ["Drahoňovice", "Lažany", "Mladostov", "Poddoubí", "Skalany"] {
            let suggestion = address_suggestion(
                json!({"city":part,"postcode":"512 64","county":"Semily"}),
                coordinates,
            );
            assert!(suggestion.is_vysker, "{part}");
            assert_eq!(suggestion.municipality.as_deref(), Some("Vyskeř"));
            assert_eq!(suggestion.locality.as_deref(), Some(part));
        }
        assert!(
            address_suggestion(
                json!({"district":"Lažany","postcode":"51264","county":"okres Semily"}),
                coordinates,
            )
            .is_vysker
        );
        for properties in [
            json!({"city":"Lažany"}),
            json!({"city":"Lažany","county":"Semily"}),
            json!({"city":"Lažany","postcode":"512 64"}),
            json!({"city":"Lažany","postcode":"679 22","county":"Semily"}),
            json!({"city":"Lažany","postcode":"51264","county":"Liberec"}),
        ] {
            assert!(
                !address_suggestion(properties.clone(), coordinates).is_vysker,
                "{properties}"
            );
        }
        assert!(
            !address_suggestion(
                json!({"city":"Lažany","postcode":"51264","county":"Semily"}),
                [16.55, 49.3],
            )
            .is_vysker
        );
    }

    #[test]
    fn does_not_infer_residence_from_a_street_name_or_conflicting_municipality() {
        for properties in [
            json!({"street":"Vyskeřská","city":"Turnov"}),
            json!({"street":"Vyskeř","city":"Praha"}),
            json!({"city":"Vyskeřská"}),
            json!({"city":"Lažany","postcode":"67922","county":"Blansko"}),
            json!({"city":"Turnov","district":"Lažany","postcode":"51264","county":"Semily"}),
            json!({"city":"Turnov","district":"Vyskeř","postcode":"51264","county":"Semily"}),
            json!({"city":"Vyskeř","municipality":"Turnov"}),
            json!({"street":"Lažany","postcode":"51264","county":"Semily"}),
        ] {
            assert!(
                !address_suggestion(properties.clone(), [15.158555, 50.5295088]).is_vysker,
                "{properties}"
            );
        }
        assert!(
            suggestions(&json!({"features":[{
                "properties":{"city":"Vyskeř","countrycode":"DE"},
                "geometry":{"type":"Point","coordinates":[15.158555,50.5295088]}
            }]}))
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn older_suggestion_payloads_do_not_claim_local_residence() {
        let suggestion: Suggestion = serde_json::from_value(json!({
            "label":"Vyskeřská 50","latitude":50.5,"longitude":15.1
        }))
        .unwrap();
        assert!(!suggestion.is_vysker);
        assert_eq!(suggestion.municipality, None);
        assert_eq!(suggestion.locality, None);
    }

    #[tokio::test]
    async fn caches_results_and_handles_upstream_failure() {
        use axum::{Router, routing::get};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let app = Router::new()
            .route(
                "/api/",
                get(
                    move |Query(query): Query<std::collections::HashMap<String, String>>| {
                        received.fetch_add(1, Ordering::SeqCst);
                        async move {
                            assert_eq!(query["countrycode"], "CZ");
                            assert_eq!(query["lat"], "50.5295088");
                            Json(json!({"features":[]}))
                        }
                    },
                ),
            )
            .route("/error", get(|| async { StatusCode::TOO_MANY_REQUESTS }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let service = AddressSearch::new(&format!("http://{address}/api/")).unwrap();
        assert!(
            service
                .lookup("Vyskeř", "http://localhost")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .lookup("VYSKEŘ", "http://localhost")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let failing = AddressSearch::new(&format!("http://{address}/error")).unwrap();
        assert_eq!(
            failing
                .lookup("Vyskeř", "http://localhost")
                .await
                .unwrap_err()
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        task.abort();
    }
}
