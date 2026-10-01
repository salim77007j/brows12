//! Engine configuration.

use brows12_layout::Viewport;

/// Privacy feature switches applied at the engine level.
#[derive(Debug, Clone)]
pub struct PrivacySettings {
    /// Network ad/tracker blocking (Brave adblock engine + default filters).
    pub block_ads: bool,
    /// Upgrade http:// to https:// automatically.
    pub https_upgrade: bool,
    /// Anti-fingerprinting: strict navigator spoofs for JS realms.
    pub strict_fingerprinting: bool,
    /// CNAME-cloaking protection (requires DoH CNAME chains).
    pub block_cname_cloaking: bool,
    /// Block third-party cookies entirely (otherwise CHIPS-partition them).
    pub block_third_party_cookies: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        PrivacySettings {
            block_ads: true,
            https_upgrade: true,
            strict_fingerprinting: false,
            block_cname_cloaking: true,
            block_third_party_cookies: false,
        }
    }
}

/// Top-level engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub user_agent: String,
    pub viewport: Viewport,
    pub privacy: PrivacySettings,
    /// Hard cap on simultaneously live (non-suspended) pages.
    pub max_live_pages: usize,
    /// Persist cookies/cache to this directory (None = in-memory profile).
    pub profile_dir: Option<std::path::PathBuf>,
    /// Log filter for the tracing subscriber (e.g. "info").
    pub log_filter: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            user_agent: brows12_net::ClientConfig::default().user_agent,
            viewport: Viewport::default(),
            privacy: PrivacySettings::default(),
            max_live_pages: 6,
            profile_dir: None,
            log_filter: None,
        }
    }
}
