use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Failure {
    pub status: u16,
    pub message: String,
}
impl Failure {
    fn network() -> Self {
        Self {
            status: 0,
            message: "Server není dostupný. Zkontrolujte připojení a zkuste to znovu.".into(),
        }
    }
}
pub type Result<T> = std::result::Result<T, Failure>;

#[derive(Clone, Deserialize)]
pub struct Session {
    pub administrator_id: i64,
    pub email: String,
    pub csrf_token: String,
    pub minimum_password_length: usize,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub published_on: Option<String>,
    pub withdraw_on: Option<String>,
    pub withdrawn_at: Option<String>,
    pub reference_number: Option<String>,
    pub issuer: Option<String>,
    pub category_id: Option<i64>,
    pub retain_attachments: bool,
    pub review_json: String,
    pub published_at: Option<String>,
    pub attachments: Vec<File>,
    pub slug: String,
    pub content: String,
    pub published: Option<bool>,
}
impl Entry {
    pub fn status(&self) -> &str {
        match self.published {
            Some(true) => "published",
            Some(false) => "draft",
            None => &self.status,
        }
    }
}
#[derive(Clone, Deserialize)]
pub struct File {
    pub id: i64,
    pub name: String,
    pub size_bytes: i64,
    pub available: bool,
    pub removed_at: Option<String>,
}
#[derive(Clone, Deserialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
}
#[derive(Clone, Default, Deserialize)]
pub struct Overview {
    pub published: i64,
    pub scheduled: i64,
    pub drafts: i64,
    pub documents: i64,
    pub pages: i64,
    pub subscribers: i64,
    pub pending_mail: i64,
    pub current_date: String,
}
#[derive(Clone, Deserialize)]
pub struct Audit {
    pub id: i64,
    pub occurred_at: String,
    pub actor_email: Option<String>,
    pub operation: String,
    pub entity_type: String,
    pub entity_id: Option<i64>,
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T> {
    request("GET", path, None, None).await
}

pub async fn save(method: &str, path: &str, data: Option<Value>) -> Result<Value> {
    // Obnoví CSRF token i po přihlášení v druhé záložce. Cookie zůstává HttpOnly.
    let session: Session = get("/api/v1/admin/session").await?;
    request(method, path, data, Some(&session.csrf_token)).await
}

pub async fn request<T: DeserializeOwned>(
    method: &str,
    path: &str,
    data: Option<Value>,
    csrf: Option<&str>,
) -> Result<T> {
    #[cfg(feature = "hydrate")]
    {
        let mut request = gloo_net::http::RequestBuilder::new(path)
            .method(method.parse().map_err(|_| Failure::network())?)
            .credentials(web_sys::RequestCredentials::SameOrigin);
        if let Some(token) = csrf {
            request = request.header("X-CSRF-Token", token);
        }
        let response = match data {
            Some(body) => {
                request
                    .json(&body)
                    .map_err(|_| Failure::network())?
                    .send()
                    .await
            }
            None => request.send().await,
        }
        .map_err(|_| Failure::network())?;
        decode(response).await
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (method, path, data, csrf);
        Err(Failure::network())
    }
}

#[cfg(feature = "hydrate")]
async fn decode<T: DeserializeOwned>(response: gloo_net::http::Response) -> Result<T> {
    let status = response.status();
    if !response.ok() {
        let message = response
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| match status {
                413 => "Soubor je příliš velký. Limit je 10 MiB.".into(),
                422 => "Některé údaje nejsou platné. Zkontrolujte formulář.".into(),
                401 => "Přihlášení vypršelo. Rozpracované údaje zůstávají ve formuláři.".into(),
                _ => "Operaci se nepodařilo dokončit. Zkuste to znovu.".into(),
            });
        return Err(Failure { status, message });
    }
    if status == 204 {
        return serde_json::from_value(Value::Null).map_err(|_| Failure::network());
    }
    response.json().await.map_err(|_| Failure {
        status: 0,
        message: "Server vrátil neočekávanou odpověď.".into(),
    })
}

pub async fn upload(path: &str, file: web_sys::File) -> Result<Value> {
    #[cfg(feature = "hydrate")]
    {
        let session: Session = get("/api/v1/admin/session").await?;
        let body = web_sys::FormData::new().map_err(|_| Failure::network())?;
        body.append_with_blob_and_filename("file", &file, &file.name())
            .map_err(|_| Failure::network())?;
        let response = gloo_net::http::Request::post(path)
            .credentials(web_sys::RequestCredentials::SameOrigin)
            .header("X-CSRF-Token", &session.csrf_token)
            .body(body)
            .map_err(|_| Failure::network())?
            .send()
            .await
            .map_err(|_| Failure::network())?;
        decode(response).await
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (path, file);
        Err(Failure::network())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kind {
    Notice,
    Document,
    Page,
}
impl Kind {
    pub fn api(self) -> &'static str {
        match self {
            Self::Notice => "notices",
            Self::Document => "documents",
            Self::Page => "pages",
        }
    }
    pub fn path(self) -> &'static str {
        match self {
            Self::Notice => "/admin/uredni-deska",
            Self::Document => "/admin/dokumenty",
            Self::Page => "/admin/stranky",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Notice => "Úřední deska",
            Self::Document => "Dokumenty",
            Self::Page => "Stránky",
        }
    }
    pub fn create_label(self) -> &'static str {
        match self {
            Self::Notice => "Nové vyvěšení",
            Self::Document => "Nový dokument",
            Self::Page => "Nová stránka",
        }
    }
}
pub fn state_label(value: &str) -> &'static str {
    match value {
        "draft" => "Koncept",
        "scheduled" => "Naplánováno",
        "published" => "Zveřejněno",
        "archived" | "withdrawn" => "Archiv",
        _ => "Neznámý stav",
    }
}
pub fn date(value: &str) -> String {
    let day = value.get(..10).unwrap_or(value);
    let parts: Vec<_> = day.split('-').collect();
    if parts.len() == 3 {
        format!(
            "{}. {}. {}",
            parts[2].trim_start_matches('0'),
            parts[1].trim_start_matches('0'),
            parts[0]
        )
    } else {
        value.into()
    }
}

#[derive(Clone, Deserialize)]
pub struct MailRecord {
    pub id: i64,
    pub subscriber_id: i64,
    pub email: String,
    pub purpose: String,
    pub subject: String,
    pub created_at: i64,
    pub sent_at: Option<i64>,
    pub attempts: i64,
    pub status: String,
}
pub fn unix_date(value: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(value)
        .ok()
        .and_then(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}
