//! Anti-fingerprinting: configuration and the defense script injected into
//! every JS realm (main frame + iframes) via the Servo UserContentManager.
//!
//! Design (Phase 4 Area 2.2):
//!
//! * **Session seed** — 64 bits of process entropy per browser run.
//! * **Per-site seed** — hash(session_seed, site hostname): noise is
//!   deterministic within a session for one site (breaks session
//!   linkability while keeping the page self-consistent) and differs
//!   across sites and across sessions.
//! * **Balanced level (default)** — aligns with Brave "Standard":
//!   canvas + audio farbling-style noise, WebGL vendor/renderer spoof,
//!   hardware caps (cores/memory/touch), font-probing jitter via
//!   measureText, Battery/Network/MediaDevices locked down, WebRTC ICE
//!   enumeration defused, gamepads/USB/BT/Serial blocked.
//! * **Strict level** — everything above plus navigator UA/platform,
//!   generic screen geometry + color depth, timezone → UTC.
//!
//! The generated script is self-contained ES5-compatible JS that patches
//! prototypes once per document before any page script runs.

use serde::{Deserialize, Serialize};

/// How aggressively to perturb fingerprintable surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpoofLevel {
    /// No spoofing (script not installed).
    Off,
    /// Brave-"Standard"-aligned: canvas/audio noise, WebGL spoof, hardware
    /// caps, font jitter, hardware enumeration + WebRTC + sensors lockdown.
    #[default]
    Balanced,
    /// Strict: Balanced + navigator UA/platform + screen + timezone.
    Strict,
}

/// Navigator-level spoofs exposed to JS realms (Strict level values).
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
            hardware_concurrency: Some(8),
            device_memory_gb: Some(8),
            max_touch_points: Some(0),
            webdriver: false,
        }
    }
}

/// Full anti-fingerprinting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintConfig {
    pub level: SpoofLevel,
    pub navigator: NavigatorSpoof,
    /// WebGL UNMASKED_RENDERER string to report.
    pub webgl_vendor: Option<String>,
    pub webgl_renderer: Option<String>,
    /// Spoofed screen dimensions (Strict).
    pub screen: Option<(u32, u32)>,
    /// Report UTC timezone (Strict).
    pub timezone_utc: bool,
}

impl Default for FingerprintConfig {
    fn default() -> Self {
        FingerprintConfig {
            level: SpoofLevel::Balanced,
            navigator: NavigatorSpoof::default(),
            webgl_vendor: Some("Intel Inc.".into()),
            webgl_renderer: Some("Intel Iris OpenGL Engine".into()),
            screen: Some((1920, 1080)),
            timezone_utc: true,
        }
    }
}

impl FingerprintConfig {
    /// The Strict preset ("maximum protection" mode).
    pub fn strict(user_agent: &str) -> Self {
        FingerprintConfig {
            level: SpoofLevel::Strict,
            navigator: NavigatorSpoof {
                user_agent: Some(user_agent.to_string()),
                platform: Some("Win32".into()),
                ..NavigatorSpoof::default()
            },
            webgl_vendor: Some("Intel Inc.".into()),
            webgl_renderer: Some("Intel Iris OpenGL Engine".into()),
            screen: Some((1920, 1080)),
            timezone_utc: true,
        }
    }

    /// 64-bit session seed from the OS entropy pool (fallback: time).
    pub fn fresh_session_seed() -> u64 {
        use std::io::Read;
        let mut buf = [0u8; 8];
        if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_ok() {
            u64::from_le_bytes(buf)
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15)
        }
    }
}

/// FNV-1a — stable per-site seed derivation (must match the JS side).
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Per-site seed: deterministic within (session, site), random across both.
pub fn per_site_seed(session_seed: u64, site: &str) -> u64 {
    let mut buf = Vec::with_capacity(site.len() + 8);
    buf.extend_from_slice(&session_seed.to_le_bytes());
    buf.extend_from_slice(site.as_bytes());
    fnv1a(&buf)
}

/// Generate the self-contained defense script for one browser session.
///
/// `session_seed` must be freshly random per process; the script derives
/// per-site seeds from it at runtime (hostnames are available in JS).
pub fn defense_script(config: &FingerprintConfig, session_seed: u64) -> String {
    if config.level == SpoofLevel::Off {
        return String::new();
    }
    let strict = config.level == SpoofLevel::Strict;
    let webgl_vendor = config.webgl_vendor.clone().unwrap_or_else(|| "Intel Inc.".into());
    let webgl_renderer =
        config.webgl_renderer.clone().unwrap_or_else(|| "Intel Iris OpenGL Engine".into());
    let ua = config.navigator.user_agent.clone().unwrap_or_default();
    let platform = config.navigator.platform.clone().unwrap_or_default();
    let langs = config.navigator.languages.join("|");
    let cores = config.navigator.hardware_concurrency.unwrap_or(8);
    let mem = config.navigator.device_memory_gb.unwrap_or(8);
    let touch = config.navigator.max_touch_points.unwrap_or(0);
    let screen = config.screen.unwrap_or((1920, 1080));

    format!(
        r#"// brows12 anti-fingerprinting (Phase 4 Area 2.2)
(function () {{
  'use strict';
  if (window.__brows12fp) {{ return; }}
  var SESSION_SEED = 0x{session_seed:016x}n;
  var STRICT = {strict};
  var WEBGL_VENDOR = {webgl_vendor:?};
  var WEBGL_RENDERER = {webgl_renderer:?};
  var NAV_UA = {ua:?};
  var NAV_PLATFORM = {platform:?};
  var NAV_LANGS = {langs:?}.split('|');
  var NAV_CORES = {cores};
  var NAV_MEM = {mem};
  var NAV_TOUCH = {touch};
  var SCREEN = [{w}, {h}];

  // --- per-site PRNG (FNV-1a + mulberry32) -------------------------------
  function fnv1a(bytes) {{
    var h = 0xcbf29ce484222325n;
    for (var i = 0; i < bytes.length; i++) {{
      h ^= BigInt(bytes[i]);
      h = BigInt.asUintN(64, h * 0x100000001b3n);
    }}
    return h;
  }}
  function siteSeed() {{
    var host = '';
    try {{ host = location.hostname || ''; }} catch (e) {{}}
    var bytes = [];
    var s = SESSION_SEED.toString(16) + '|' + host;
    for (var i = 0; i < s.length; i++) {{ bytes.push(s.charCodeAt(i) & 0xff); }}
    return fnv1a(bytes);
  }}
  function mulberry32(a) {{
    return function () {{
      a |= 0; a = (a + 0x6D2B79F5) | 0;
      var t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    }};
  }}
  var rngCache = null;
  function rng() {{
    if (!rngCache) {{ rngCache = mulberry32(Number(siteSeed() & 0xffffffffn)); }}
    return rngCache();
  }}
  function byteAt(i) {{
    // deterministic 0..255 per (site, index)
    var r = mulberry32((Number(siteSeed() & 0xffffffffn) ^ (i * 0x9E3779B1)) >>> 0)();
    return (r * 255) | 0;
  }}

  var def = Object.defineProperty;
  var stats = {{ canvas: 0, audio: 0, audioHooked: 0, webgl: 0, measure: 0 }};
  window.__brows12fp = {{ stats: stats, strict: STRICT }};

  // --- Canvas 2D noise ----------------------------------------------------
  function noiseImageData(imageData) {{
    var d = imageData.data;
    if (!d || !d.length) {{ return imageData; }}
    for (var i = 3; i < d.length; i += 4) {{
      if (d[i] !== 0) {{
        d[i] = (d[i] + byteAt(i % 97)) & 0xff;
      }}
    }}
    stats.canvas++;
    return imageData;
  }}
  function noisyCopy(canvas) {{
    var c = document.createElement('canvas');
    c.width = canvas.width; c.height = canvas.height;
    var ctx = c.getContext('2d');
    ctx.drawImage(canvas, 0, 0);
    var img = ctx.getImageData(0, 0, c.width, c.height);
    ctx.putImageData(noiseImageData(img), 0, 0);
    return c;
  }}
  function exportNoisy(origFn, canvas, args) {{
    try {{
      var c = noisyCopy(canvas);
      return origFn.apply(c, args);
    }} catch (e) {{ return origFn.apply(canvas, args); }}
  }}
  function exportNoisyBlob(origFn, canvas, args) {{
    try {{
      var c = noisyCopy(canvas);
      return origFn.apply(c, args);
    }} catch (e) {{ return origFn.apply(canvas, args); }}
  }}

  // --- AudioContext noise -------------------------------------------------
  function patchAudio(proto, method) {{
    if (!proto || typeof proto[method] !== 'function') {{ return; }}
    var orig = proto[method];
    proto[method] = function () {{
      var res = orig.apply(this, arguments);
      try {{
        if (res && res.length) {{
          for (var i = 0; i < res.length; i++) {{
            if (res[i] !== 0) {{ res[i] += (byteAt(i % 53) - 127) / 127 * 1e-7; }}
          }}
          stats.audio++;
        }}
      }} catch (e) {{}}
      return res;
    }};
    stats.audioHooked++;
  }}

  // AudioBuffer.getChannelData returns a LIVE typed array: pages write
  // into it and read back, so one-shot mutation is overwritten. Wrap the
  // returned array in a Proxy whose index reads carry the per-site noise
  // (writes pass through, so normal audio code keeps working).
  function patchChannelData() {{
    if (typeof AudioBuffer === 'undefined' || !AudioBuffer.prototype
        || typeof AudioBuffer.prototype.getChannelData !== 'function') {{ return; }}
    var orig = AudioBuffer.prototype.getChannelData;
    window.__brows12diag = {{ audioOriginalType: typeof orig, patchApplied: true }};
    AudioBuffer.prototype.getChannelData = function () {{
      window.__brows12diag.audioCalled = true;
      var res = orig.apply(this, arguments);
      if (res && res.length) {{
        stats.audio++;
        return new Proxy(res, {{
          get: function (t, k) {{
            if (typeof k === 'string' && k.length && k[0] >= '0' && k[0] <= '9'
                && !isNaN(Number(k))) {{
              var v = t[k];
              return v !== 0 ? v + (byteAt((Number(k) % 53)) - 127) / 127 * 1e-7 : v;
            }}
            var v = t[k];
            return typeof v === 'function' ? v.bind(t) : v;
          }},
          set: function (t, k, val) {{ t[k] = val; return true; }}
        }});
      }}
      return res;
    }};
    stats.audioHooked++;
  }}

  // --- WebGL spoof + readPixels noise ------------------------------------
  function patchWebGL(proto) {{
    if (!proto) {{ return; }}
    var origGetParameter = proto.getParameter;
    proto.getParameter = function (p) {{
      // UNMASKED_VENDOR_WEBGL / UNMASKED_RENDERER_WEBGL
      if (p === 0x9245) {{ stats.webgl++; return WEBGL_VENDOR; }}
      if (p === 0x9246) {{ stats.webgl++; return WEBGL_RENDERER; }}
      if (p === 0x1F00) {{ return WEBGL_VENDOR; }}   // VENDOR
      if (p === 0x1F01) {{ return WEBGL_RENDERER; }} // RENDERER
      return origGetParameter.apply(this, arguments);
    }};
    if (proto.readPixels) {{
      var origRead = proto.readPixels;
      proto.readPixels = function () {{
        origRead.apply(this, arguments);
        try {{
          var pixels = arguments[6];
          if (pixels && pixels.length) {{
            for (var i = 3; i < pixels.length; i += 4) {{
              if (pixels[i] !== 0) {{ pixels[i] = (pixels[i] + byteAt(i % 89)) & 0xff; }}
            }}
            stats.webgl++;
          }}
        }} catch (e) {{}}
      }};
    }}
  }}

  // --- measureText jitter (font probing) ----------------------------------
  function patchMeasure(proto) {{
    if (!proto || !proto.measureText) {{ return; }}
    var orig = proto.measureText;
    proto.measureText = function (text) {{
      var m = orig.apply(this, arguments);
      try {{
        var jitter = ((byteAt(String(text).length) - 127) / 127) * 0.6;
        var props = ['width', 'actualBoundingBoxLeft', 'actualBoundingBoxRight'];
        for (var i = 0; i < props.length; i++) {{
          if (typeof m[props[i]] === 'number') {{
            def(m, props[i], {{ value: m[props[i]] + jitter, configurable: true }});
          }}
        }}
        stats.measure++;
      }} catch (e) {{}}
      return m;
    }};
  }}

  // --- Hardware caps + navigator surfaces ---------------------------------
  function patchNavigator(nav) {{
    if (!nav) {{ return; }}
    try {{ def(nav, 'hardwareConcurrency', {{ get: function () {{ return NAV_CORES; }}, configurable: true }}); }} catch (e) {{}}
    try {{ if ('deviceMemory' in nav || NAV_MEM) def(nav, 'deviceMemory', {{ get: function () {{ return NAV_MEM; }}, configurable: true }}); }} catch (e) {{}}
    try {{ def(nav, 'maxTouchPoints', {{ get: function () {{ return NAV_TOUCH; }}, configurable: true }}); }} catch (e) {{}}
    try {{ def(nav, 'webdriver', {{ get: function () {{ return false; }}, configurable: true }}); }} catch (e) {{}}
    try {{ def(nav, 'languages', {{ get: function () {{ return NAV_LANGS.slice(); }}, configurable: true }}); }} catch (e) {{}}
    try {{ def(nav, 'language', {{ get: function () {{ return NAV_LANGS[0]; }}, configurable: true }}); }} catch (e) {{}}
    if (STRICT && NAV_UA) {{
      try {{ def(nav, 'userAgent', {{ get: function () {{ return NAV_UA; }}, configurable: true }}); }} catch (e) {{}}
      try {{ def(nav, 'appVersion', {{ get: function () {{ return NAV_UA.replace(/^Mozilla\//, ''); }}, configurable: true }}); }} catch (e) {{}}
    }}
    if (STRICT && NAV_PLATFORM) {{
      try {{ def(nav, 'platform', {{ get: function () {{ return NAV_PLATFORM; }}, configurable: true }}); }} catch (e) {{}}
    }}
    // Plugins: report the canonical Chrome PDF pair (empty looks anomalous).
    try {{
      var fakePlugin = {{ name: 'PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format', length: 1 }};
      def(nav, 'plugins', {{ get: function () {{ return [fakePlugin, fakePlugin, fakePlugin]; }}, configurable: true }});
    }} catch (e) {{}}
  }}

  // --- Sensors & hardware enumeration -------------------------------------
  function patchEnumeration(nav) {{
    if (!nav) {{ return; }}
    try {{ nav.getBattery = function () {{ return Promise.resolve({{
      charging: true, chargingTime: 0, dischargingTime: Infinity, level: 1,
      addEventListener: function () {{}}, removeEventListener: function () {{}}
    }}); }}; }} catch (e) {{}}
    try {{
      def(nav, 'connection', {{ get: function () {{ return {{
        effectiveType: '4g', downlink: 10, downlinkMax: 100, rtt: 50,
        saveData: false, type: 'wifi', onchange: null
      }}; }}, configurable: true }}); }} catch (e) {{}}
    try {{ if (nav.mediaDevices) nav.mediaDevices.enumerateDevices = function () {{ return Promise.resolve([]); }}; }} catch (e) {{}}
    try {{ nav.getGamepads = function () {{ return [null, null, null, null]; }}; }} catch (e) {{}}
    try {{ if (nav.usb) nav.usb.getDevices = function () {{ return Promise.resolve([]); }}; }} catch (e) {{}}
    try {{ if (nav.bluetooth) nav.bluetooth.getAvailability = function () {{ return false; }}; }} catch (e) {{}}
    try {{ if (nav.serial) nav.serial.getPorts = function () {{ return Promise.resolve([]); }}; }} catch (e) {{}}
    try {{ if (nav.requestMIDIAccess) nav.requestMIDIAccess = function () {{ return Promise.reject(new Error('blocked')); }}; }} catch (e) {{}}
  }}

  // --- WebRTC leak guard ---------------------------------------------------
  function patchWebRTC() {{
    var PC = window.RTCPeerConnection || window.webkitRTCPeerConnection;
    if (!PC) {{ return; }}
    function StubPC() {{
      var listeners = {{}};
      this.createDataChannel = function () {{ return {{ close: function () {{}} }}; }};
      this.createOffer = function () {{ return Promise.resolve({{ type: 'offer', sdp: '' }}); }};
      this.createAnswer = function () {{ return Promise.resolve({{ type: 'answer', sdp: '' }}); }};
      this.setLocalDescription = function () {{ return Promise.resolve(); }};
      this.setRemoteDescription = function () {{ return Promise.resolve(); }};
      this.addIceCandidate = function () {{ return Promise.resolve(); }};
      this.close = function () {{}};
      this.addEventListener = function (t, f) {{ (listeners[t] = listeners[t] || []).push(f); }};
      this.removeEventListener = function () {{}};
      // Local (mDNS-style obfuscated) candidates only — never srflx.
      def(this, 'localDescription', {{ get: function () {{
        return {{ type: 'offer', sdp: 'v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\n' }};
      }} }});
      def(this, 'iceConnectionState', {{ get: function () {{ return 'new'; }} }});
      def(this, 'signalingState', {{ get: function () {{ return 'stable'; }} }});
    }}
    StubPC.prototype = PC.prototype;
    window.RTCPeerConnection = StubPC;
    window.webkitRTCPeerConnection = StubPC;
  }}

  // --- Strict-only surfaces ------------------------------------------------
  function patchScreen() {{
    if (!STRICT) {{ return; }}
    var scr = window.screen;
    if (scr && SCREEN.length === 2) {{
      try {{
        def(scr, 'width', {{ get: function () {{ return SCREEN[0]; }}, configurable: true }});
        def(scr, 'height', {{ get: function () {{ return SCREEN[1]; }}, configurable: true }});
        def(scr, 'availWidth', {{ get: function () {{ return SCREEN[0]; }}, configurable: true }});
        def(scr, 'availHeight', {{ get: function () {{ return SCREEN[1] - 40; }}, configurable: true }});
        def(scr, 'availLeft', {{ get: function () {{ return 0; }}, configurable: true }});
        def(scr, 'availTop', {{ get: function () {{ return 0; }}, configurable: true }});
        def(scr, 'colorDepth', {{ get: function () {{ return 24; }}, configurable: true }});
        def(scr, 'pixelDepth', {{ get: function () {{ return 24; }}, configurable: true }});
      }} catch (e) {{}}
    }}
    try {{ def(window, 'devicePixelRatio', {{ get: function () {{ return 1; }}, configurable: true }}); }} catch (e) {{}}
  }}
  function patchTimezone() {{
    if (!STRICT) {{ return; }}
    try {{
      var OrigDTF = Intl.DateTimeFormat;
      var patched = function () {{
        var locale = arguments[0];
        var opts = (arguments.length >= 2 && arguments[1]) ? Object.assign({{}}, arguments[1]) : {{}};
        opts.timeZone = 'UTC';
        return new OrigDTF(locale, opts);
      }};
      patched.prototype = OrigDTF.prototype;
      patched.supportedLocalesOf = OrigDTF.supportedLocalesOf;
      Intl.DateTimeFormat = patched;
    }} catch (e) {{}}
    try {{
      var origOffset = Date.prototype.getTimezoneOffset;
      Date.prototype.getTimezoneOffset = function () {{ return 0; }};
    }} catch (e) {{}}
  }}

  // --- install -------------------------------------------------------------
  try {{
    var origGetContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (type) {{
      var ctx = origGetContext.apply(this, arguments);
      if (ctx && String(type) === '2d'
          && !CanvasRenderingContext2D.prototype.__brows12Patched) {{
        patchCanvas2DOnly(CanvasRenderingContext2D.prototype);
        CanvasRenderingContext2D.prototype.__brows12Patched = true;
      }}
      return ctx;
    }};
  }} catch (e) {{}}
  function patchCanvas2DOnly(proto) {{
    var origGet = proto.getImageData;
    if (origGet) {{
      proto.getImageData = function () {{
        return noiseImageData(origGet.apply(this, arguments));
      }};
    }}
  }}
  try {{ if (typeof HTMLCanvasElement !== 'undefined') {{
    var origToURL = HTMLCanvasElement.prototype.toDataURL;
    HTMLCanvasElement.prototype.toDataURL = function () {{ return exportNoisy(origToURL, this, arguments); }};
    var origToBlob = HTMLCanvasElement.prototype.toBlob;
    HTMLCanvasElement.prototype.toBlob = function () {{
      var args = Array.prototype.slice.call(arguments);
      return exportNoisyBlob(origToBlob, this, args);
    }};
  }} }} catch (e) {{}}
  try {{ patchChannelData(); }} catch (e) {{}}
  try {{ patchAudio(AnalyserNode.prototype, 'getFloatTimeDomainData'); }} catch (e) {{}}
  try {{ patchAudio(AnalyserNode.prototype, 'getFloatFrequencyData'); }} catch (e) {{}}
  try {{ patchWebGL(window.WebGLRenderingContext && WebGLRenderingContext.prototype); }} catch (e) {{}}
  try {{ patchWebGL(window.WebGL2RenderingContext && WebGL2RenderingContext.prototype); }} catch (e) {{}}
  try {{ patchMeasure(CanvasRenderingContext2D.prototype); }} catch (e) {{}}
  try {{ patchNavigator(window.navigator); }} catch (e) {{}}
  try {{ patchEnumeration(window.navigator); }} catch (e) {{}}
  try {{ patchWebRTC(); }} catch (e) {{}}
  try {{ patchScreen(); }} catch (e) {{}}
  try {{ patchTimezone(); }} catch (e) {{}}
}})();"#,
        session_seed = session_seed,
        strict = strict,
        webgl_vendor = webgl_vendor,
        webgl_renderer = webgl_renderer,
        ua = ua,
        platform = platform,
        langs = langs,
        cores = cores,
        mem = mem,
        touch = touch,
        w = screen.0,
        h = screen.1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_site_seed_is_deterministic_per_session_and_site() {
        let a = per_site_seed(42, "https://site.com");
        let b = per_site_seed(42, "https://site.com");
        assert_eq!(a, b);
        let c = per_site_seed(42, "https://other.com");
        assert_ne!(a, c);
        let d = per_site_seed(43, "https://site.com");
        assert_ne!(a, d, "seed must differ across sessions");
    }

    #[test]
    fn script_contains_core_hooks() {
        let cfg = FingerprintConfig::default();
        let s = defense_script(&cfg, 0x1234_5678_9abc_def0);
        assert!(s.contains("__brows12fp"));
        assert!(s.contains("getImageData"));
        assert!(s.contains("toDataURL"));
        assert!(s.contains("getChannelData"));
        assert!(s.contains("0x9246"), "UNMASKED_RENDERER_WEBGL");
        assert!(s.contains("measureText"));
        assert!(s.contains("RTCPeerConnection"));
        assert!(s.contains("enumerateDevices"));
        assert!(s.contains("getBattery"));
        assert!(s.contains("123456789abcdef0"), "session seed embedded");
    }

    #[test]
    fn strict_only_surfaces_gated() {
        let mut cfg = FingerprintConfig::default();
        cfg.level = SpoofLevel::Balanced;
        let balanced = defense_script(&cfg, 1);
        assert!(balanced.contains("STRICT = false"));
        cfg.level = SpoofLevel::Strict;
        let strict = defense_script(&cfg, 1);
        assert!(strict.contains("STRICT = true"));
        assert!(strict.contains("getTimezoneOffset"));
    }

    #[test]
    fn off_generates_empty_script() {
        let mut cfg = FingerprintConfig::default();
        cfg.level = SpoofLevel::Off;
        assert!(defense_script(&cfg, 1).is_empty());
    }

    #[test]
    fn fresh_seed_is_random() {
        let a = FingerprintConfig::fresh_session_seed();
        let b = FingerprintConfig::fresh_session_seed();
        assert_ne!(a, b, "two fresh seeds must differ (urandom)");
    }

    #[test]
    fn strict_preset_covers_surfaces() {
        let cfg = FingerprintConfig::strict("TestUA/1.0");
        assert_eq!(cfg.level, SpoofLevel::Strict);
        assert!(cfg.navigator.user_agent.is_some());
        assert_eq!(cfg.webgl_renderer.as_deref(), Some("Intel Iris OpenGL Engine"));
    }
}
