# Privacy

Brows12's privacy model is engine-level: protections run before pages can
observe anything, and they are on by default.

## Network blocking

- Brave's `adblock` engine (the same engine Brave ships) with uBlock
  Origin-compatible filter syntax. The embedded default list
  (`privacy/src/default_filters.txt`) is a curated subset of EasyList +
  EasyPrivacy covering the highest-volume ad and analytics networks
  (doubleclick, googlesyndication, GA/GTM exceptions included, Taboola,
  Criteo, Hotjar, Mixpanel, Segment, Amplitude, clarity, and more).
- Blocking happens **before** any connection: blocked documents render the
  built-in shield page; blocked subresources never leave the process.
- Filter lists are plain text — full EasyList/EasyPrivacy can be loaded at
  runtime with `PrivacyBlocker::from_filters`.
- Every block emits a `RequestBlocked` event with the matched filter for UI
  shield counters.

## Cookie protection

- **CHIPS-style partitioning**: third-party contexts only see cookies whose
  `Partitioned` key matches the top-level site (schemeful, registrable
  domain via the Public Suffix List). First-party contexts never see
  partitioned cookies from other sites.
- Optional strict mode (`block_third_party_cookies`) rejects third-party
  cookies outright.
- Public-suffix enforcement: `Set-Cookie: Domain=co.uk` is rejected.
- HttpOnly cookies are hidden from `document.cookie`; Secure cookies never
  travel over plaintext.

## HTTPS upgrade + HSTS

- `http://` navigations are rewritten to `https://` (loopback exempt).
- HSTS entries (max-age, includeSubDomains) are honored with expiry.

## CNAME cloaking defense

First-party trackers hide behind CNAME records that point a site's
subdomain at a third-party analytics host. Brows12 resolves via its DoH
client, extracts the CNAME chain, and compares registrable domains: a
chain that crosses sites flags the request (`CnameVerdict::Cloaked`),
feedable into the same blocking path.

## Anti-fingerprinting

Engine hooks inject per-origin spoofs into every JS realm
(`brows12_privacy::FingerprintConfig`):

- Navigator: userAgent, platform, languages, hardwareConcurrency,
  deviceMemory, maxTouchPoints, `webdriver = false`, empty plugin list.
- Canvas noise: deterministic per-origin seed (stable within a session,
  different across sites) so canvas reads are perturbed without breaking
  page rendering.
- WebGL vendor/renderer spoof strings; AudioContext entropy blocking;
  hardware enumeration (gamepads/USB/Bluetooth) rejection; WebRTC ICE
  enumeration disabled (leak protection independent of a WebRTC stack).
- `:visited` never matches in the selector engine — no history sniffing.

`SpoofLevel::Strict` (`FingerprintConfig::strict`) applies the full preset;
the default `Balanced` ships navigator + hardware restrictions without
canvas perturbation (which can alter page appearance).

## Telemetry

There is none. The engine has no update pings, no metrics, no crash
reporting, no experiments framework. `tracing` output is local and
opt-in (`log_filter` in `EngineConfig`).
