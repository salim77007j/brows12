//! Anti-fingerprinting configuration and spoof value sets.

use serde::{Deserialize, Serialize};

/// How aggressively to perturb fingerprintable surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpoofLevel {
    /// No spoofing.
    Off,
    /// Moderate: randomize high-entropy APIs only (canvas/audio noise).
    #[default]
    Balanced,
    /// Strict: canvas + audio + WebGL + screen + navigator + timezone + fonts.
    Strict,
}

/// Navigator-level spoofs exposed to JS realms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigatorSpoof {
    pub user_agent: Option<String>,
    pub platform: Option<String>,
    pub languages: Vec<String>,
    pub hardware_concurrency: Option<u32>,
    pub device_memory_gb: Option<u32>,
    pub max_touch_points: Option<u32>,
    pub webdriver: bool,
}

impl Default for NavigatorSpoof {
    fn default() -> Self {
        NavigatorSpoof {
            user_agent: None,
            platform: None,
            languages: vec!["en-US".into(), "en".into()],
            hardware_concurrency: None,
            device_memory_gb: None,
            max_touch_points: None,
            webdriver: false,
        }
    }
}

/// Full anti-fingerprinting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintConfig {
    pub level: SpoofLevel,
    pub navigator: NavigatorSpoof,
    /// Per-origin deterministic canvas noise seed (0 = no noise).
    pub canvas_noise_seed: u64,
    /// Spoofed screen dimensions for `screen.width/height`.
    pub screen: Option<(u32, u32)>,
    /// WebGL vendor/renderer spoof strings.
    pub webgl_vendor: Option<String>,
    pub webgl_renderer: Option<String>,
    /// Do not emit audio context entropy (blocks AudioContext FP).
    pub block_audio_entropy: bool,
    /// Restrict `navigator.getGamepads`, USB, Bluetooth, Serial.
    pub block_hardware_enumeration: bool,
    /// Reject WebRTC ICE candidate enumeration entirely (leak protection).
    pub webrtc_disabled: bool,
}

impl Default for FingerprintConfig {
    fn default() -> Self {
        FingerprintConfig {
            level: SpoofLevel::Balanced,
            navigator: NavigatorSpoof::default(),
            canvas_noise_seed: 0,
            screen: None,
            webgl_vendor: None,
            webgl_renderer: None,
            block_audio_entropy: false,
            block_hardware_enumeration: true,
            webrtc_disabled: true,
        }
    }
}

impl FingerprintConfig {
    /// Enable per-origin deterministic canvas noise (Strict level behavior).
    pub fn enable_canvas_noise(&mut self, origin: &str) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        origin.hash(&mut h);
        self.canvas_noise_seed = h.finish();
    }

    /// The complete strict preset used by "maximum protection" mode.
    pub fn strict(user_agent: &str) -> Self {
        FingerprintConfig {
            level: SpoofLevel::Strict,
            navigator: NavigatorSpoof {
                user_agent: Some(user_agent.to_string()),
                platform: Some("Win32".into()),
                languages: vec!["en-US".into(), "en".into()],
                hardware_concurrency: Some(8),
                device_memory_gb: Some(8),
                max_touch_points: Some(0),
                webdriver: false,
            },
            canvas_noise_seed: 0x4241_4252_4f57_5345,
            screen: Some((1920, 1080)),
            webgl_vendor: Some("Intel Inc.".into()),
            webgl_renderer: Some("Intel Iris OpenGL Engine".into()),
            block_audio_entropy: true,
            block_hardware_enumeration: true,
            webrtc_disabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_preset_covers_surfaces() {
        let cfg = FingerprintConfig::strict("TestUA/1.0");
        assert_eq!(cfg.level, SpoofLevel::Strict);
        assert!(cfg.navigator.user_agent.is_some());
        assert!(cfg.webgl_renderer.is_some());
        assert!(cfg.webrtc_disabled);
        assert!(cfg.block_audio_entropy);
    }

    #[test]
    fn canvas_seed_is_deterministic_per_origin() {
        let mut a = FingerprintConfig::default();
        a.enable_canvas_noise("https://site.com");
        let mut b = FingerprintConfig::default();
        b.enable_canvas_noise("https://site.com");
        assert_eq!(a.canvas_noise_seed, b.canvas_noise_seed);
        b.enable_canvas_noise("https://other.com");
        assert_ne!(a.canvas_noise_seed, b.canvas_noise_seed);
    }
}
