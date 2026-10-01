//! Environment services injected into every JS realm.

use brows12_net::HttpClient;
use brows12_privacy::fingerprint::NavigatorSpoof;
use brows12_storage::cookies::CookieJar;
use brows12_storage::WebStorage;
use std::sync::{Arc, Mutex};

/// Everything a JS realm can reach outside its sandbox.
#[derive(Clone)]
pub struct JsEnvironment {
    pub net: Arc<HttpClient>,
    /// Tokio runtime used to spawn network work (fetch).
    pub tokio: Arc<tokio::runtime::Runtime>,
    /// localStorage for this origin.
    pub local_storage: WebStorage,
    /// Cookie jar (shared with the network stack).
    pub cookies: Arc<Mutex<CookieJar>>,
    /// Anti-fingerprinting navigator values.
    pub navigator: NavigatorSpoof,
    /// Base URL of the document (location object).
    pub base_url: String,
    /// CHIPS partition key (registrable domain of the top-level site).
    pub top_level_site: String,
    /// Viewport in CSS pixels.
    pub viewport: (u32, u32),
    /// Captured console output (for tests + UI consoles).
    pub console_log: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for JsEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsEnvironment")
            .field("base_url", &self.base_url)
            .field("viewport", &self.viewport)
            .finish()
    }
}

impl JsEnvironment {
    /// Location parsed into components (for the `location` object).
    pub fn location_parts(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut out = serde_json::Map::new();
        if let Ok(url) = url::Url::parse(&self.base_url) {
            let mut set = |k: &str, v: String| {
                out.insert(k.to_string(), serde_json::Value::String(v));
            };
            set("href", url.to_string());
            set("protocol", format!("{}:", url.scheme()));
            set(
                "host",
                url.host_str()
                    .map(|h| match url.port() {
                        Some(p) => format!("{h}:{p}"),
                        None => h.to_string(),
                    })
                    .unwrap_or_default(),
            );
            set("hostname", url.host_str().unwrap_or("").to_string());
            set("port", url.port().map(|p| p.to_string()).unwrap_or_default());
            set("pathname", url.path().to_string());
            set("search", url.query().map(|q| format!("?{q}")).unwrap_or_default());
            set("hash", url.fragment().map(|f| format!("#{f}")).unwrap_or_default());
            let host_with_port = match (url.host_str(), url.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_string(),
                (None, _) => String::new(),
            };
            set("origin", format!("{}://{}", url.scheme(), host_with_port));
        }
        out
    }

    /// Serialize the navigator spoof into a JSON object.
    pub fn navigator_parts(&self) -> serde_json::Map<String, serde_json::Value> {
        let nav = &self.navigator;
        let mut out = serde_json::Map::new();
        out.insert(
            "userAgent".into(),
            serde_json::Value::String(nav.user_agent.clone().unwrap_or_else(|| "Brows12".into())),
        );
        out.insert(
            "platform".into(),
            serde_json::Value::String(
                nav.platform.clone().unwrap_or_else(|| "Linux x86_64".into()),
            ),
        );
        out.insert(
            "language".into(),
            serde_json::Value::String(
                nav.languages.first().cloned().unwrap_or_else(|| "en-US".into()),
            ),
        );
        out.insert("languages".into(), serde_json::json!(nav.languages));
        out.insert("webdriver".into(), serde_json::Value::Bool(nav.webdriver));
        out.insert(
            "hardwareConcurrency".into(),
            nav.hardware_concurrency.map(|v| serde_json::json!(v)).unwrap_or(serde_json::json!(4)),
        );
        out.insert(
            "deviceMemory".into(),
            nav.device_memory_gb.map(|v| serde_json::json!(v)).unwrap_or(serde_json::json!(8)),
        );
        out.insert(
            "maxTouchPoints".into(),
            nav.max_touch_points.map(|v| serde_json::json!(v)).unwrap_or(serde_json::json!(0)),
        );
        out.insert("doNotTrack".into(), serde_json::Value::String("1".into()));
        out.insert("plugins".into(), serde_json::json!([]));
        out
    }
}
