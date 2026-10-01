//! The engine: subsystem construction + shared services.

use crate::config::EngineConfig;
use brows12_net::{ClientConfig, CookieStore, HttpClient};
use brows12_privacy::{HttpsUpgrader, PrivacyBlocker};
use brows12_storage::cache::HttpCache;
use brows12_storage::cookies::CookieJar;
use brows12_storage::{KeyValueStore, MemoryStore};
use std::sync::{Arc, Mutex};

/// Events emitted by the engine for the UI layer.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    NavigationStarted { tab: u64, url: String },
    NavigationCommitted { tab: u64, url: String },
    FrameReady { tab: u64, generation: u64 },
    LoadFinished { tab: u64, title: String },
    RequestBlocked { tab: u64, url: String, reason: String },
    Console { tab: u64, message: String },
    Suspended { tab: u64 },
    Resumed { tab: u64 },
}

/// Cookie jar adapter implementing the network layer's CookieStore trait.
pub(crate) struct JarAdapter(pub Arc<Mutex<CookieJar>>);

impl CookieStore for JarAdapter {
    fn header_for(
        &self,
        url: &url::Url,
        top_level_site: &str,
        is_third_party: bool,
    ) -> Option<String> {
        let jar = self.0.lock().ok()?;
        jar.header_for_url(url, top_level_site, is_third_party)
    }

    fn record(&self, url: &url::Url, set_cookies: &[String], top_level_site: &str) {
        if let Ok(mut jar) = self.0.lock() {
            for sc in set_cookies {
                jar.set_from_header(url, sc, top_level_site);
            }
        }
    }
}

/// Adapts [`PrivacyBlocker`] to the network stack's `PolicyFilter` seam so
/// that every outbound request — page subresources, JS `fetch`, WebSocket —
/// passes the ad/tracker filter exactly once, inside the client.
struct BlockerPolicy(Arc<PrivacyBlocker>);

impl brows12_net::client::PolicyFilter for BlockerPolicy {
    fn allow(&self, url: &url::Url, _resource_type: &str) -> Result<(), String> {
        let host = url.host_str().unwrap_or_default();
        match self.0.check(url.as_str(), host, brows12_privacy::blocker::RequestKind::Other) {
            Some(reason) => Err(format!("{reason:?}")),
            None => Ok(()),
        }
    }
}

/// The engine handle. Cheap to clone; all subsystems are shared.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<EngineInner>,
}

pub struct EngineInner {
    pub config: EngineConfig,
    pub tokio: Arc<tokio::runtime::Runtime>,
    pub net: Arc<HttpClient>,
    pub cookies: Arc<Mutex<CookieJar>>,
    pub blocker: Arc<PrivacyBlocker>,
    pub upgrader: Arc<HttpsUpgrader>,
    pub cache: Arc<HttpCache>,
    pub kv: Arc<dyn KeyValueStore>,
    pub fonts: Arc<Mutex<cosmic_text::FontSystem>>,
    pub events: tokio::sync::broadcast::Sender<EngineEvent>,
    pub next_tab_id: Mutex<u64>,
    pub tabs: Mutex<Vec<std::sync::Weak<crate::tab::Tab>>>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").field("version", &env!("CARGO_PKG_VERSION")).finish()
    }
}

impl Engine {
    /// Build an engine from configuration.
    pub fn new(config: EngineConfig) -> Self {
        let net_config =
            ClientConfig { user_agent: config.user_agent.clone(), ..ClientConfig::default() };

        let jar = Arc::new(Mutex::new(CookieJar::new()));
        let net = Arc::new(HttpClient::new(net_config, Some(Arc::new(JarAdapter(jar.clone())))));

        let blocker = Arc::new(PrivacyBlocker::new());
        let upgrader = Arc::new(HttpsUpgrader::default());
        let cache = Arc::new(HttpCache::new());
        let kv: Arc<dyn KeyValueStore> = Arc::new(MemoryStore::new());

        // Install the ad/tracker filter at the network layer: it now covers
        // pipeline fetches AND JS-initiated fetch/WebSocket connections.
        net.set_policy(Arc::new(BlockerPolicy(blocker.clone())));

        let (events, _rx) = tokio::sync::broadcast::channel(256);

        let tokio = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime"),
        );

        if let Some(filter) = &config.log_filter {
            let _ = tracing_subscriber_handle(filter);
        }

        Engine {
            inner: Arc::new(EngineInner {
                config,
                tokio,
                net,
                cookies: jar,
                blocker,
                upgrader,
                cache,
                kv,
                fonts: Arc::new(Mutex::new(cosmic_text::FontSystem::new())),
                events,
                next_tab_id: Mutex::new(0),
                tabs: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Subscribe to engine events (UI layer consumes these).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    pub fn config(&self) -> &EngineConfig {
        &self.inner.config
    }

    /// Blocking handle into the engine's network runtime for ad-hoc fetches
    /// (used by the UI for downloads, preconnects, etc.).
    pub fn http(&self) -> &HttpClient {
        &self.inner.net
    }

    pub fn cookie_jar(&self) -> &Arc<Mutex<CookieJar>> {
        &self.inner.cookies
    }

    pub fn blocker(&self) -> &PrivacyBlocker {
        &self.inner.blocker
    }

    pub fn cache(&self) -> &HttpCache {
        &self.inner.cache
    }

    /// Human-readable engine identity (User-Agent, About pages).
    pub fn user_agent(&self) -> &str {
        &self.inner.config.user_agent
    }

    /// Create a new tab (registered with the engine for budget management).
    pub fn tab(&self) -> Arc<crate::tab::Tab> {
        let mut next = self.inner.next_tab_id.lock().unwrap();
        *next += 1;
        let tab = Arc::new(crate::tab::Tab::new(*next, self.inner.clone()));
        self.inner.tabs.lock().unwrap().push(Arc::downgrade(&tab));
        tab
    }

    /// Suspend the least-recently-used live tabs beyond the page budget.
    pub fn enforce_memory_budget(&self) {
        enforce_budget(&self.inner);
    }
}

/// Engine-wide LRU sweep (callable from tabs during loads).
pub(crate) fn enforce_budget(inner: &Arc<EngineInner>) {
    {
        let tabs = inner.tabs.lock().unwrap();
        let mut live: Vec<(u64, Arc<crate::tab::Tab>)> = tabs
            .iter()
            .filter_map(|w| w.upgrade())
            .filter(|t| t.is_live())
            .map(|t| (t.last_used_secs(), t))
            .collect();
        live.sort_by_key(|(used, _)| *used);
        let budget = inner.config.max_live_pages.max(1);
        let keep = live.len().saturating_sub(budget);
        for (_, tab) in live.iter().take(keep) {
            tab.suspend();
        }
    }
}

impl Engine {
    /// Number of live (resident) pages.
    pub fn live_page_count(&self) -> usize {
        self.inner
            .tabs
            .lock()
            .unwrap()
            .iter()
            .filter(|w| w.upgrade().map(|t| t.is_live()).unwrap_or(false))
            .count()
    }
}

fn tracing_subscriber_handle(filter: &str) -> Result<(), String> {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(filter).map_err(|e| e.to_string())?)
        .try_init()
        .map_err(|e| e.to_string())
}
