//! Cookie policy, WebRTC policy and CNAME-cloaking verdicts.

use brows12_storage::registrable_domain;
use serde::{Deserialize, Serialize};

/// Cookie handling policy applied by the net + storage layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CookiePolicy {
    /// Accept all first-party cookies; partition third-party (CHIPS).
    #[default]
    PartitionThirdParty,
    /// Block all third-party cookies outright.
    BlockThirdParty,
    /// Accept everything (dangerous; opt-in only).
    AllowAll,
}

/// WebRTC network policy (leak protection even before a WebRTC stack exists).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WebRtcPolicy {
    /// WebRTC fully disabled at the engine level.
    #[default]
    Disabled,
    /// Only local/public host candidates; no mDNS, no relay enumeration.
    PublicOnly,
    /// Default browser behavior.
    Default,
}

/// Verdict for a host whose CNAME chain was resolved via DoH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CnameVerdict {
    /// Direct resolution, nothing suspicious.
    Clean,
    /// The host is a CNAME alias into a different registrable domain —
    /// classic CNAME cloaking used to bypass third-party cookie blocking.
    Cloaked { alias_target: String, target_site: String },
}

/// Runtime policy bundle owned by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PolicyEngine {
    pub cookies: CookiePolicy,
    pub webrtc: WebRtcPolicy,
    pub block_cname_cloaking: bool,
}

impl PolicyEngine {
    /// Classify a CNAME chain (ordered: final host first).
    pub fn classify_cname(&self, host: &str, chain: &[String]) -> CnameVerdict {
        if !self.block_cname_cloaking || chain.is_empty() {
            return CnameVerdict::Clean;
        }
        let host_site = registrable_domain(host);
        for target in chain {
            let target_site = registrable_domain(target);
            if !target_site.is_empty() && target_site != host_site {
                return CnameVerdict::Cloaked { alias_target: target.clone(), target_site };
            }
        }
        CnameVerdict::Clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cname_cloaking() {
        let engine = PolicyEngine {
            cookies: CookiePolicy::PartitionThirdParty,
            webrtc: WebRtcPolicy::Disabled,
            block_cname_cloaking: true,
        };
        // site.com points at analytics.io (cloaking).
        let v = engine.classify_cname("data.site.com", &["analytics-tracker.io".to_string()]);
        assert_eq!(
            v,
            CnameVerdict::Cloaked {
                alias_target: "analytics-tracker.io".into(),
                target_site: "analytics-tracker.io".into()
            }
        );
    }

    #[test]
    fn same_site_cname_is_clean() {
        let engine = PolicyEngine { block_cname_cloaking: true, ..Default::default() };
        let v = engine.classify_cname("cdn.site.com", &["assets.site.com".to_string()]);
        assert_eq!(v, CnameVerdict::Clean);
    }

    #[test]
    fn disabled_check_is_clean() {
        let engine = PolicyEngine { block_cname_cloaking: false, ..Default::default() };
        let v = engine.classify_cname("a.com", &["evil.io".to_string()]);
        assert_eq!(v, CnameVerdict::Clean);
    }
}
