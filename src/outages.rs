//! Read-only synchronization status shown in calendar administration.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutageSyncStatus {
    pub enabled: bool,
    pub town_code: String,
    pub interval_seconds: u64,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub next_attempt_at: Option<String>,
    pub last_error: Option<String>,
}
