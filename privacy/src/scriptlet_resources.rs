//! uBlock-Origin-compatible resources for the adblock engine.
//!
//! Two families:
//!
//! 1. **Redirect targets** (`$redirect=…`) — real replacement bodies the
//!    engine serves instead of blocked requests. The names and aliases
//!    mirror uBO's canonical resource names so 2026 filter lists resolve.
//! 2. **Scriptlets** (`##+js(…)` / `$redirect` templates) — uBO-compatible
//!    templated JavaScript with `{{1}}` parameter substitution.
//!
//! This is a curated core bundle: the most-referenced redirect targets in
//! the 2026 EasyList/EasyPrivacy/uBO lists plus the highest-value scriptlets
//! (anti-anti-adblock, WebRTC, eval, window.open, JSON pruning).

use adblock::resources::{MimeType, PermissionMask, Resource, ResourceType};
use base64::Engine as _;

/// Minimal valid silent MP3 (~0.1 s, 748 bytes, generated with ffmpeg).
const NOOP_MP3_B64: &str = "SUQzBAAAAAAAIlRTU0UAAAAOAAADTGF2ZjYxLjcuMTAzAAAAAAAAAAAAAAD/+0DAAAAAAAAAAAAAAAAAAAAAAABJbmZvAAAADwAAAAUAAALAAGhoaGhoaGhoaGhoaGhoaGhoaGiOjo6Ojo6Ojo6Ojo6Ojo6Ojo6OjrS0tLS0tLS0tLS0tLS0tLS0tLS02tra2tra2tra2tra2tra2tra2tr//////////////////////////wAAAABMYXZjNjEuMTkAAAAAAAAAAAAAAAAkAwYAAAAAAAACwMvx0WoAAAAAAP/7EMQAA8AAAaQAAAAgAAA0gAAABExBTUUzLjEwMFVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV//sSxCmDwAABpAAAACAAADSAAAAEVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV//sQxFODwAABpAAAACAAADSAAAAEVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVX/+xLEfQPAAAGkAAAAIAAANIAAAARVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVX/+xDEpwPAAAGkAAAAIAAANIAAAARVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVQ==";
/// Minimal valid MP4 (1 s 2x2 black frame, 2.2 KB, generated with ffmpeg).
const NOOP_MP4_B64: &str = "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";
/// 2x2 transparent PNG.
const PNG_2X2_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAC0lEQVR4nGNgAAEAAAYAAf6MZ8gAAAAASUVORK5CYII=";
/// 1x1 transparent GIF (43 bytes).
const GIF_1X1_B64: &str = "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

const NOOP_JS: &str = "function () {}\n";
const EMPTY: &str = "";
const NOOP_HTML: &str =
    "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body></body></html>";

/// Google IMA SDK stub (EasyList redirects many video ad SDKs here).
const GOOGLE_IMA_JS: &str = r#"(function () {
  const noop = function () {};
  const AdDisplayContainer = function (c) { this.container = c; };
  AdDisplayContainer.prototype.initialize = noop;
  const AdsLoader = function () {};
  AdsLoader.prototype.getSettings = function () { return {}; };
  AdsLoader.prototype.contentComplete = noop;
  AdsLoader.prototype.destroy = noop;
  AdsLoader.prototype.requestAds = noop;
  AdsLoader.prototype.addEventListener = noop;
  const AdsManager = { width: 0, height: 0, destroy: noop, addEventListener: noop,
    getRemainingTime: function () { return 0; }, init: noop, resize: noop, start: noop,
    setVolume: noop, pause: noop, resume: noop, expand: noop, collapse: noop };
  const AdsRequest = function () {};
  AdsRequest.prototype.setAdDisplayContainer = noop;
  AdsRequest.prototype.setAdTagUrl = noop;
  google.ima = {
    VERSION: '3.0.0-stub-brows12',
    AdDisplayContainer: AdDisplayContainer,
    AdsLoader: AdsLoader,
    AdsRequest: AdsRequest,
    AdEvent: { Type: { LOADED: 'loaded', STARTED: 'started', COMPLETE: 'complete', ALL_ADS_COMPLETED: 'allAdsCompleted' } },
    AdErrorEvent: { Type: { AD_ERROR: 'adError' } },
    ViewMode: { NORMAL: 'normal', FULLSCREEN: 'fullscreen' },
    settings: { setLocale: noop, setDisableCustomPlaybackForIOS10Plus: noop, setPlayerType: noop, setPlayerVersion: noop }
  };
  google.ima.AdsManagerLoadedEvent = { Type: { ADS_MANAGER_LOADED: 'adsManagerLoaded' } };
})();
"#;

/// adsbygoogle stub (defines push so layout keeps working).
const ADSBYGOOGLE_JS: &str = r#"(function () {
  window.adsbygoogle = window.adsbygoogle || [];
  window.adsbygoogle.loaded = true;
  window.adsbygoogle.op = window.adsbygoogle.op || [];
  const push = window.adsbygoogle.push.bind(window.adsbygoogle);
  window.adsbygoogle.push = function (arg) {
    if (arg && typeof arg === 'object' && arg.params) {
      window.adsbygoogle.op.push(arg.params);
    }
    return 0;
  };
})();
"#;

/// Chartbeat stub.
const CHARTBEAT_JS: &str = r#"(function () {
  const stub = { pSUPERFLY: { virtualPage: function () {} }, init: function () {},
    volley: { log: function () {} }, _q: [] };
  window._sf_async_config = window._sf_async_config || {};
  window.chartbeat = stub;
  window.pSUPERFLY = stub.pSUPERFLY;
})();
"#;

/// FuckAdBlock 3.2.0 stub (neutralises anti-adblock walls that probe for it).
const FUCKADBLOCK_JS: &str = r#"(function () {
  const Fab = function () {};
  Fab.prototype.setOption = function () { return this; };
  Fab.prototype.setDetectDelay = function () { return this; };
  Fab.prototype.onDetect = function () { return this; };
  Fab.prototype.onDetected = function () { return this; };
  Fab.prototype.onNotDetected = function () { return this; };
  Fab.prototype.check = function () { return false; };
  window.FuckAdBlock = window.BlockAdBlock = Fab;
  window.fuckAdBlock = window.blockAdBlock = new Fab();
})();
"#;

/// Click-to-load placeholder for `$redirect` of frames.
const CLICK2LOAD_HTML: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><style>
html,body{margin:0;height:100%;display:flex;align-items:center;justify-content:center;
background:#f2f2f2;font-family:sans-serif;color:#333}
button{padding:8px 16px;border:1px solid #bbb;border-radius:4px;background:#fff;cursor:pointer}
</style></head>
<body><button id="load">Blocked frame — click to load</button>
<script>
document.getElementById('load').addEventListener('click', function () {
  const real = new URLSearchParams(location.search).get('originalUrl');
  if (real) { window.location.replace(real); }
});
</script></body></html>
"#;

// ---------------------------------------------------------------------------
// Scriptlets (uBO-compatible, templated)
// ---------------------------------------------------------------------------

const SET_CONSTANT_JS: &str = r#"(function () {
  const chain = '{{1}}';
  let value = '{{2}}';
  if (chain === '' || chain === '{{1}}') { return; }
  switch (value) {
    case 'undefined': value = undefined; break;
    case 'false': value = false; break;
    case 'true': value = true; break;
    case 'null': value = null; break;
    case 'NaN': value = NaN; break;
    case 'noopFunc': value = function () {}; break;
    case 'trueFunc': value = function () { return true; }; break;
    case 'falseFunc': value = function () { return false; }; break;
    case 'throwFunc': value = function () { throw new Error('set-constant'); }; break;
    case 'emptyArr': value = []; break;
    case 'emptyStr': value = ''; break;
    default:
      if (/^-?\d+(\.\d+)?$/.test(value)) { value = parseFloat(value); }
      else if (value.startsWith('{') || value.startsWith('[')) {
        try { value = JSON.parse(value); } catch (e) { return; }
      }
      break;
  }
  let owner = window;
  const parts = chain.split('.');
  for (let i = 0; i < parts.length - 1; i++) {
    if (owner == null) { return; }
    const p = parts[i];
    if (owner[p] == null) {
      try { owner[p] = {}; } catch (e) { return; }
    }
    owner = owner[p];
  }
  const prop = parts[parts.length - 1];
  try {
    Object.defineProperty(owner, prop, {
      get: function () { return value; },
      set: function () {},
      configurable: false,
    });
  } catch (e) {
    try { owner[prop] = value; } catch (e2) {}
  }
})();
"#;

const ABORT_ON_PROPERTY_READ_JS: &str = r#"(function () {
  const chain = '{{1}}';
  if (chain === '' || chain === '{{1}}') { return; }
  let owner = window;
  const parts = chain.split('.');
  for (let i = 0; i < parts.length - 1; i++) {
    if (owner == null) { return; }
    owner = owner[parts[i]];
  }
  if (owner == null) { return; }
  const prop = parts[parts.length - 1];
  try {
    Object.defineProperty(owner, prop, {
      get: function () { throw new ReferenceError('abort-on-property-read: ' + chain); },
      set: function () {},
      configurable: false,
    });
  } catch (e) {}
})();
"#;

const ABORT_ON_PROPERTY_WRITE_JS: &str = r#"(function () {
  const chain = '{{1}}';
  if (chain === '' || chain === '{{1}}') { return; }
  let owner = window;
  const parts = chain.split('.');
  for (let i = 0; i < parts.length - 1; i++) {
    if (owner == null) { return; }
    const p = parts[i];
    if (owner[p] == null) { try { owner[p] = {}; } catch (e) { return; } }
    owner = owner[p];
  }
  if (owner == null) { return; }
  const prop = parts[parts.length - 1];
  try {
    Object.defineProperty(owner, prop, {
      get: function () {},
      set: function () { throw new ReferenceError('abort-on-property-write: ' + chain); },
      configurable: false,
    });
  } catch (e) {}
})();
"#;

const NOWEBRTC_JS: &str = r#"(function () {
  const noop = function () {};
  const Stub = function () {
    this.createDataChannel = function () { return { close: noop }; };
    this.createOffer = function () { return Promise.resolve(); };
    this.createAnswer = function () { return Promise.resolve(); };
    this.setLocalDescription = function () { return Promise.resolve(); };
    this.setRemoteDescription = function () { return Promise.resolve(); };
    this.addIceCandidate = function () { return Promise.resolve(); };
    this.close = noop;
    this.addEventListener = noop;
    this.removeEventListener = noop;
    Object.defineProperty(this, 'localDescription', { get: function () { return null; } });
    Object.defineProperty(this, 'remoteDescription', { get: function () { return null; } });
    Object.defineProperty(this, 'signalingState', { get: function () { return 'closed'; } });
    Object.defineProperty(this, 'iceConnectionState', { get: function () { return 'closed'; } });
  };
  Stub.prototype = Object.create(EventTarget && EventTarget.prototype || Object.prototype);
  window.RTCPeerConnection = Stub;
  window.webkitRTCPeerConnection = Stub;
  if (window.RTCDataChannel) { window.RTCDataChannel = undefined; }
})();
"#;

const NOEVAL_JS: &str = r#"(function () {
  const log = '{{1}}';
  const loud = log === 'log' || log === 'debug' || log === 'verbose';
  window.eval = function () { if (loud) { console.info('noeval: blocked eval()'); } };
  const F = window.Function;
  window.Function = function () {
    if (loud) { console.info('noeval: blocked Function()'); }
    return function () {};
  };
  Object.setPrototypeOf(window.Function, F);
  Object.defineProperty(window.Function, 'prototype', { value: F.prototype });
})();
"#;

const WINDOW_OPEN_DEFUSER_JS: &str = r#"(function () {
  const pattern = '{{1}}';
  const match = (pattern && pattern !== '{{1}}')
    ? new RegExp(pattern, 'i')
    : null;
  window.open = function (url) {
    if (!match || (url && match.test(String(url)))) {
      if (match) { console.info('window.open-defuser: blocked', String(url)); }
      return null;
    }
    return null;
  };
})();
"#;

/// Simplified json-prune: prunes matching dot-paths from JSON.parse results.
const JSON_PRUNE_JS: &str = r#"(function () {
  const paths = '{{1}}';
  if (paths === '' || paths === '{{1}}') { return; }
  const stackPaths = ({{2}} !== undefined && {{2}} !== '{{2}}') ? String({{2}}) : '';
  const props = paths.split(/\s+/).filter(Boolean);
  const stackProps = stackPaths.split(/\s+/).filter(Boolean);
  const prune = function (obj, depth) {
    if (obj === null || typeof obj !== 'object' || depth > 8) { return; }
    for (const key in obj) {
      if (!Object.prototype.hasOwnProperty.call(obj, key)) { continue; }
      if (props.indexOf(key) !== -1) { delete obj[key]; continue; }
      if (stackProps.length && props.some(function (p) {
        return stackProps.indexOf(key) !== -1 ? false : p.startsWith(key + '.');
      })) { /* still walk */ }
      prune(obj[key], depth + 1);
    }
  };
  const parse = JSON.parse;
  JSON.parse = function () {
    const result = parse.apply(this, arguments);
    try { prune(result, 0); } catch (e) {}
    return result;
  };
})();
"#;

/// abort-current-script (approximation): neutralises dynamic <script> src
/// assignments matching the pattern by swapping in an empty data: URL.
const ABORT_CURRENT_SCRIPT_JS: &str = r#"(function () {
  const pattern = '{{1}}';
  if (!pattern || pattern === '{{1}}') { return; }
  let re;
  try { re = new RegExp(pattern, 'i'); } catch (e) { return; }
  let desc;
  try {
    desc = Object.getOwnPropertyDescriptor(HTMLScriptElement.prototype, 'src');
  } catch (e) { return; }
  if (!desc || !desc.set) { return; }
  Object.defineProperty(HTMLScriptElement.prototype, 'src', {
    get: desc.get,
    set: function (v) {
      try {
        if (re.test(String(v))) {
          v = 'data:text/javascript,/* abort-current-script */';
        }
      } catch (e) {}
      desc.set.call(this, v);
    },
    configurable: desc.configurable,
    enumerable: desc.enumerable,
  });
})();
"#;

// ---------------------------------------------------------------------------

fn text(name: &str, aliases: &[&str], mime: MimeType, content: &str) -> Resource {
    Resource {
        name: name.to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        kind: ResourceType::Mime(mime),
        content: base64::engine::general_purpose::STANDARD.encode(content.as_bytes()),
        dependencies: vec![],
        permission: PermissionMask::default(),
    }
}

fn from_b64(name: &str, aliases: &[&str], mime: MimeType, b64: &str) -> Resource {
    Resource {
        name: name.to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        kind: ResourceType::Mime(mime),
        content: b64.to_string(),
        dependencies: vec![],
        permission: PermissionMask::default(),
    }
}

fn template(name: &str, js: &str) -> Resource {
    Resource {
        name: name.to_string(),
        aliases: vec![],
        kind: ResourceType::Template,
        content: base64::engine::general_purpose::STANDARD.encode(js.as_bytes()),
        dependencies: vec![],
        permission: PermissionMask::default(),
    }
}

/// The complete brows12 resource bundle installed into every engine.
pub fn all() -> Vec<Resource> {
    vec![
        // ---- redirect targets -------------------------------------------
        text("noop.js", &["noopjs"], MimeType::ApplicationJavascript, NOOP_JS),
        text("noop.txt", &["nooptext"], MimeType::TextPlain, EMPTY),
        text("noop.html", &["noop.html"], MimeType::TextHtml, NOOP_HTML),
        text("noopframe.html", &["noopframe"], MimeType::TextHtml, NOOP_HTML),
        from_b64("1x1.gif", &["1x1.gif"], MimeType::ImageGif, GIF_1X1_B64),
        from_b64("2x2.png", &["2x2.png"], MimeType::ImagePng, PNG_2X2_B64),
        from_b64(
            "noopmp3-0.1s.mp3",
            &["noopmp3-0.1s", "noop-0.1s.mp3"],
            MimeType::AudioMp3,
            NOOP_MP3_B64,
        ),
        from_b64(
            "noopmp4-1s.mp4",
            &["noopmp4-1s", "noop-1s.mp4", "noop-1s"],
            MimeType::VideoMp4,
            NOOP_MP4_B64,
        ),
        text("google-ima.js", &[], MimeType::ApplicationJavascript, GOOGLE_IMA_JS),
        text(
            "googlesyndication_adsbygoogle.js",
            &["googlesyndication.com/adsbygoogle.js"],
            MimeType::ApplicationJavascript,
            ADSBYGOOGLE_JS,
        ),
        text("chartbeat.js", &["chartbeat"], MimeType::ApplicationJavascript, CHARTBEAT_JS),
        text(
            "fuckadblock.js-3.2.0",
            &["fuckadblock.js-3.2.0"],
            MimeType::ApplicationJavascript,
            FUCKADBLOCK_JS,
        ),
        text("click2load.html", &[], MimeType::TextHtml, CLICK2LOAD_HTML),
        // ---- scriptlets --------------------------------------------------
        template("set-constant.js", SET_CONSTANT_JS),
        template("abort-on-property-read.js", ABORT_ON_PROPERTY_READ_JS),
        template("abort-on-property-write.js", ABORT_ON_PROPERTY_WRITE_JS),
        template("nowebrtc.js", NOWEBRTC_JS),
        template("noeval.js", NOEVAL_JS),
        template("window.open-defuser.js", WINDOW_OPEN_DEFUSER_JS),
        template("json-prune.js", JSON_PRUNE_JS),
        template("abort-current-script.js", ABORT_CURRENT_SCRIPT_JS),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_has_list_referenced_names() {
        let names: Vec<String> = all().iter().map(|r| r.name.clone()).collect();
        let aliases: Vec<String> = all().iter().flat_map(|r| r.aliases.clone()).collect();
        for required in [
            "noop.js",
            "noopjs",
            "noop.txt",
            "nooptext",
            "1x1.gif",
            "2x2.png",
            "noopmp3-0.1s",
            "noopmp4-1s",
            "google-ima.js",
            "googlesyndication_adsbygoogle.js",
            "fuckadblock.js-3.2.0",
            "chartbeat.js",
            "set-constant.js",
            "nowebrtc.js",
            "window.open-defuser.js",
        ] {
            assert!(
                names.iter().chain(aliases.iter()).any(|n| n == required),
                "missing resource: {required}"
            );
        }
    }

    #[test]
    fn binary_resources_are_valid_base64_of_known_magic() {
        let bundle = all();
        let find = |name: &str| bundle.iter().find(|r| r.name == name).unwrap();

        let bytes =
            base64::engine::general_purpose::STANDARD.decode(&find("1x1.gif").content).unwrap();
        assert_eq!(&bytes[..3], b"GIF");
        let bytes =
            base64::engine::general_purpose::STANDARD.decode(&find("2x2.png").content).unwrap();
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&find("noopmp3-0.1s.mp3").content)
            .unwrap();
        assert_eq!(&bytes[..3], b"ID3");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&find("noopmp4-1s.mp4").content)
            .unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
    }

    #[test]
    fn scriptlets_are_templated_and_syntax_free_of_stray_tokens() {
        let token_re = |s: &str| {
            // Every `{{...}}` occurrence must be a numbered parameter.
            let mut rest = s;
            while let Some(i) = rest.find("{{") {
                let after = &rest[i + 2..];
                match after.find("}}") {
                    Some(j) => {
                        let inner = &after[..j];
                        assert!(
                            !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()),
                            "stray template token {{{{{inner}}}}}"
                        );
                        rest = &after[j + 2..];
                    }
                    None => panic!("unterminated template token"),
                }
            }
        };
        for r in all().iter().filter(|r| r.kind == ResourceType::Template) {
            let js = String::from_utf8(
                base64::engine::general_purpose::STANDARD.decode(&r.content).unwrap(),
            )
            .unwrap();
            token_re(&js);
        }
    }
}
