use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};
use tokio::sync::{Mutex, RwLock};

#[derive(Clone)]
pub struct Session {
    pub token: String,
    pub user: User,
    pub refresh: Option<Refresh>,
}

#[derive(Clone)]
pub struct Refresh {
    pub token: String,
    pub device: String,
    pub client: String,
    pub expires: Instant,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    pub name: String,
    pub avatar: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    // VK uses negative owner IDs for communities.
    pub id: i64,
    pub name: String,
    pub avatar: Option<String>,
    pub community: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Photos,
    Videos,
    Documents,
    Profile,
    Wall,
    Friends,
    Messages,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub owner_id: i64,
    pub kind: Kind,
    pub title: String,
    pub count: u64,
    pub cover: Option<String>,
    pub description: String,
    pub available: bool,
    pub error: Option<String>,
    #[serde(skip)]
    pub api_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub duration: Option<u64>,
    pub date: Option<i64>,
    pub downloadable: bool,
    pub reason: Option<String>,
    #[serde(skip)]
    pub url: Option<String>,
    #[serde(skip)]
    pub player: Option<String>,
    #[serde(skip)]
    pub extension: String,
    #[serde(skip)]
    pub raw: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub items: Vec<Media>,
    pub total: u64,
    pub next_offset: Option<u64>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub collection_id: String,
    // None means the entire collection, including all subsequent API pages.
    pub item_ids: Option<Vec<String>>,
    #[serde(default)]
    pub excluded_ids: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub id: String,
    pub status: String,
    pub phase: String,
    pub total: usize,
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub unavailable: usize,
    pub bytes: u64,
    pub current: String,
    pub current_bytes: u64,
    pub current_total: Option<u64>,
    pub speed: f64,
    pub folder: String,
    pub errors: Vec<String>,
    pub started_at: u64,
}

pub struct Job {
    pub progress: Mutex<Progress>,
    pub paused: AtomicBool,
    pub canceled: AtomicBool,
}

pub struct AppState {
    #[cfg(test)]
    pub api_base: String,
    pub client: reqwest::Client,
    pub session: RwLock<Option<Session>>,
    pub sources: RwLock<HashMap<i64, Source>>,
    pub collections: RwLock<HashMap<String, Collection>>,
    pub limiter: Mutex<Instant>,
    pub job: Mutex<Option<Arc<Job>>>,
    pub login: Mutex<Option<Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            #[cfg(test)]
            api_base: "https://api.vk.com/method".into(),
            client: reqwest::Client::builder()
                .user_agent("VKBackup/0.1")
                .connect_timeout(std::time::Duration::from_secs(20))
                .read_timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() > 5 || !crate::backup::safe_remote(attempt.url()) {
                        attempt.stop()
                    } else {
                        attempt.follow()
                    }
                }))
                .build()
                .expect("HTTP client"),
            session: RwLock::new(None),
            sources: RwLock::new(HashMap::new()),
            collections: RwLock::new(HashMap::new()),
            limiter: Mutex::new(Instant::now()),
            job: Mutex::new(None),
            login: Mutex::new(None),
        }
    }

    pub async fn busy(&self) -> bool {
        let job = self.job.lock().await;
        if let Some(job) = job.as_ref() {
            matches!(
                job.progress.lock().await.status.as_str(),
                "running" | "paused" | "canceling"
            )
        } else {
            false
        }
    }
}
