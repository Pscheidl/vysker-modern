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
        let text = |key| {
            properties
                .get(key)
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
                .map(str::trim)
                .unwrap_or("")
        };
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
        let item = Suggestion {
            label,
            latitude,
            longitude,
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
                longitude: 15.158555
            }]
        );
        assert!(suggestions(&json!({"error":"unavailable"})).is_err());
        let town = json!({"properties":{"name":"Vyskeř","city":"Vyskeř","countrycode":"CZ"},"geometry":{"type":"Point","coordinates":[15.158555,50.5295088]}});
        assert_eq!(
            suggestions(&json!({"features":[town]})).unwrap()[0].label,
            "Vyskeř"
        );
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
