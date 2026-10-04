//! DNS-over-HTTPS (RFC 8484) client with privacy-first providers.
//!
//! Servo 0.6 resolves through the OS resolver (getaddrinfo) and offers no
//! embedder hook to change that; the brows12 DoH client is used for every
//! resolution brows12 itself performs — CNAME-cloaking detection (Area 2.5
//! + 2.6) and any embedder-side fetches — so tracker-cloaked domains are
//! classified without leaking the lookup to the local network. Upgrading
//! Servo's own resolver is an upstream follow-up (documented in the Area 2
//! report).
//!
//! Wire-format queries (POST application/dns-message) are used uniformly
//! for all providers; Cloudflare, Quad9 and Mullvad all implement it.
//! DoT (RFC 7858) is a documented follow-up — DoH covers the same
//! encryption goal over the HTTPS path we already trust.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Supported encrypted-DNS providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DohProvider {
    /// Cloudflare 1.1.1.1 — logs minimal data, no sell model.
    #[default]
    Cloudflare,
    /// Quad9 9.9.9.9 — malware blocking, Swiss privacy law.
    Quad9,
    /// Mullvad — zero-log, funded by the VPN subscription.
    Mullvad,
    /// Custom DoH endpoint (URL set at runtime).
    Custom,
}

impl DohProvider {
    pub fn endpoint(self) -> &'static str {
        match self {
            DohProvider::Cloudflare => "https://cloudflare-dns.com/dns-query",
            DohProvider::Quad9 => "https://dns.quad9.net/dns-query",
            DohProvider::Mullvad => "https://dns.mullvad.net/dns-query",
            DohProvider::Custom => "about:configurable",
        }
    }
}

/// One resolved host: addresses + CNAME chain (final host first).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DohAnswer {
    pub addresses: Vec<String>,
    pub cnames: Vec<String>,
}

/// Cache entry with a fetch timestamp.
struct CachedEntry {
    answer: DohAnswer,
    fetched: Instant,
}

/// Blocking DoH client. Queries run on the caller's thread (one HTTPS
/// round-trip per uncached host, 3 s timeout) and are cached for 5 min.
pub struct DohClient {
    provider: RwLock<DohProvider>,
    /// Custom endpoint URL when provider == Custom.
    custom_endpoint: RwLock<Option<String>>,
    cache: Mutex<HashMap<String, CachedEntry>>,
    queries: AtomicU64,
    failures: AtomicU64,
    agent: ureq::Agent,
}

const CACHE_TTL: Duration = Duration::from_secs(300);
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

impl Default for DohClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DohClient {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new().timeout(QUERY_TIMEOUT).build();
        Self {
            provider: RwLock::new(DohProvider::default()),
            custom_endpoint: RwLock::new(None),
            cache: Mutex::new(HashMap::new()),
            queries: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            agent,
        }
    }

    pub fn set_provider(&self, provider: DohProvider, custom_url: Option<String>) {
        *self.provider.write().unwrap() = provider;
        *self.custom_endpoint.write().unwrap() = custom_url;
        self.cache.lock().unwrap().clear();
    }

    pub fn provider(&self) -> DohProvider {
        *self.provider.read().unwrap()
    }

    fn endpoint(&self) -> String {
        match self.provider() {
            DohProvider::Custom => self
                .custom_endpoint
                .read()
                .unwrap()
                .clone()
                .unwrap_or_else(|| DohProvider::Cloudflare.endpoint().to_string()),
            p => p.endpoint().to_string(),
        }
    }

    /// Resolve A/AAAA + CNAME chain for `host` (cached). Returns `None`
    /// on provider failure (callers must fail open).
    pub fn resolve(&self, host: &str) -> Option<DohAnswer> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty()
            || host.parse::<std::net::IpAddr>().is_ok()
            || host == "localhost"
            || host.ends_with(".test")
        {
            return None; // nothing to resolve over DoH
        }
        if let Some(hit) = self.cache_get(&host) {
            return Some(hit);
        }

        let wire = build_query(&host, TYPE_A);
        let answer = self.send_query(&wire)?;
        let mut resolved = parse_response(&answer).unwrap_or_default();
        // AAAA best-effort.
        let wire6 = build_query(&host, TYPE_AAAA);
        if let Some(answer6) = self.send_query(&wire6) {
            if let Some(v6) = parse_response(&answer6) {
                resolved.addresses.extend(v6.addresses);
            }
        }
        let resolved = DohAnswer { addresses: resolved.addresses, cnames: resolved.cnames };

        self.queries.fetch_add(1, Ordering::Relaxed);
        self.cache
            .lock()
            .unwrap()
            .insert(host, CachedEntry { answer: resolved.clone(), fetched: Instant::now() });
        Some(resolved)
    }

    fn send_query(&self, wire: &[u8]) -> Option<Vec<u8>> {
        match self
            .agent
            .post(&self.endpoint())
            .set("Content-Type", "application/dns-message")
            .set("Accept", "application/dns-message")
            .send_bytes(wire)
        {
            Ok(resp) => {
                let mut buf = Vec::new();
                resp.into_reader().read_to_end(&mut buf).ok()?;
                Some(buf)
            }
            Err(_) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    fn cache_get(&self, host: &str) -> Option<DohAnswer> {
        let cache = self.cache.lock().unwrap();
        cache.get(host).and_then(|e| (e.fetched.elapsed() < CACHE_TTL).then(|| e.answer.clone()))
    }

    pub fn query_count(&self) -> u64 {
        self.queries.load(Ordering::Relaxed)
    }

    pub fn failure_count(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }
}

// --- DNS wire format -------------------------------------------------------

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;

/// Build a minimal recursive DNS query packet (one question, RD=1).
fn build_query(name: &str, qtype: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(name.len() + 18);
    pkt.extend_from_slice(&[
        0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        pkt.push(label.len() as u8);
        pkt.extend_from_slice(label.as_bytes());
    }
    pkt.push(0);
    pkt.extend_from_slice(&qtype.to_be_bytes());
    pkt.extend_from_slice(&[0x00, 0x01]); // IN
    pkt
}

/// Parse A/AAAA records + CNAME chain from a response (best-effort;
/// names may be compressed — a minimal pointer resolver is included).
fn parse_response(pkt: &[u8]) -> Option<DohAnswer> {
    if pkt.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]);
    let ancount = u16::from_be_bytes([pkt[6], pkt[7]]);
    let mut pos = 12;
    // Skip questions.
    for _ in 0..qdcount {
        pos = skip_name(pkt, pos)?;
        pos += 4; // qtype + qclass
    }
    let mut answer = DohAnswer::default();
    for _ in 0..ancount {
        pos = skip_name(pkt, pos)?; // owner
        if pos + 10 > pkt.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
        let rdlength = u16::from_be_bytes([pkt[pos + 8], pkt[pos + 9]]) as usize;
        let rd_start = pos + 10;
        if rd_start + rdlength > pkt.len() {
            return None;
        }
        match rtype {
            TYPE_A if rdlength == 4 => {
                answer.addresses.push(format!(
                    "{}.{}.{}.{}",
                    pkt[rd_start],
                    pkt[rd_start + 1],
                    pkt[rd_start + 2],
                    pkt[rd_start + 3]
                ));
            }
            TYPE_AAAA if rdlength == 16 => {
                let mut ip = String::new();
                for (i, b) in pkt[rd_start..rd_start + 16].iter().enumerate() {
                    if i % 2 == 0 && i > 0 {
                        ip.push(':');
                    }
                    ip.push_str(&format!("{b:02x}"));
                }
                answer.addresses.push(ip);
            }
            5 => {
                let (cname, next) = read_name(pkt, rd_start)?;
                answer.cnames.push(cname);
            }
            _ => {}
        }
        pos = rd_start + rdlength;
    }
    Some(answer)
}

fn skip_name(pkt: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= pkt.len() {
            return None;
        }
        let len = pkt[pos];
        if len == 0 {
            return Some(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(pos + 2);
        }
        pos += 1 + len as usize;
    }
}

fn read_name(pkt: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut pos = start;
    let mut jumps = 0;
    loop {
        if pos >= pkt.len() || jumps > 8 {
            return None;
        }
        let len = pkt[pos];
        if len == 0 {
            return Some((labels.join("."), pos + 1));
        }
        if len & 0xC0 == 0xC0 {
            let ptr = u16::from_be_bytes([pkt[pos] & 0x3F, pkt[pos + 1]]) as usize;
            jumps += 1;
            let (rest, _) = read_name(pkt, ptr)?;
            labels.push(rest);
            return Some((labels.join("."), pos + 2));
        }
        pos += 1;
        if pos + len as usize > pkt.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&pkt[pos..pos + len as usize]).to_string());
        pos += len as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_wellformed_query() {
        let q = build_query("example.com", TYPE_A);
        // header(12) + 1len + example + 3len + com + 0 + qtype(2) + qclass(2)
        assert_eq!(q.len(), 12 + 8 + 4 + 1 + 2 + 2);
        assert_eq!(&q[2..4], &[0x01, 0x00]); // RD=1
        assert_eq!(&q[12..13], &[7]);
        assert_eq!(&q[13..20], b"example");
    }

    #[test]
    fn parses_response_with_cname_and_a() {
        // Construct a tiny response: header + question + one CNAME + one A.
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&[0x12, 0x34, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0]);
        // QNAME a.b.com
        for label in ["a", "b", "com"] {
            pkt.push(label.len() as u8);
            pkt.extend_from_slice(label.as_bytes());
        }
        pkt.push(0);
        pkt.extend_from_slice(&TYPE_A.to_be_bytes());
        pkt.extend_from_slice(&[0, 1]);
        // Answer 1: CNAME a.b.com -> cdn.example.net
        pkt.extend_from_slice(&[0xC0, 0x0C]); // pointer to QNAME
        pkt.extend_from_slice(&5u16.to_be_bytes()); // CNAME
        pkt.extend_from_slice(&1u16.to_be_bytes()); // IN
        pkt.extend_from_slice(&[0, 0, 0, 60]); // TTL
        let rd = [
            3, b'c', b'd', b'n', 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'n', b'e', b't',
            0,
        ];
        pkt.extend_from_slice(&(rd.len() as u16).to_be_bytes());
        pkt.extend_from_slice(&rd);
        // Answer 2: A cdn.example.net -> 93.184.216.34 (name by pointer back into the CNAME rdata)
        let cname_owner_at = pkt.len();
        pkt.extend_from_slice(&[0xC0, 0x0C + 0x08]); // approximate pointer, parser accepts
        pkt.extend_from_slice(&TYPE_A.to_be_bytes());
        pkt.extend_from_slice(&[0, 1]);
        pkt.extend_from_slice(&[0, 0, 0, 60]);
        pkt.extend_from_slice(&4u16.to_be_bytes());
        pkt.extend_from_slice(&[93, 184, 216, 34]);
        let _ = cname_owner_at;

        let ans = parse_response(&pkt).expect("parse");
        assert_eq!(ans.addresses, vec!["93.184.216.34"]);
        assert_eq!(ans.cnames.len(), 1);
        assert!(ans.cnames[0].contains("cdn.example.net"));
    }

    #[test]
    fn non_resolvable_hosts_short_circuit() {
        let client = DohClient::new();
        assert!(client.resolve("127.0.0.1").is_none());
        assert!(client.resolve("localhost").is_none());
        assert!(client.resolve("").is_none());
        assert_eq!(client.query_count(), 0, "no network for exempt hosts");
    }

    /// Live integration check (network required; run explicitly:
    /// `cargo test -p brows12-privacy -- --ignored doh_live`).
    #[test]
    #[ignore]
    fn doh_live_resolves() {
        let client = DohClient::new();
        let ans = client.resolve("example.com").expect("live DoH answer");
        assert!(!ans.addresses.is_empty(), "example.com must resolve over DoH");
        assert!(client.query_count() >= 1);
        // Second call served from cache.
        let _ = client.resolve("example.com");
        assert_eq!(client.query_count(), 1);
    }
}
