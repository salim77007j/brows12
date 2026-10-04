//! RFC 6265bis cookie jar with CHIPS-style partitioning.

use crate::registrable_domain;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// How third-party cookies are handled (Phase 4 Area 2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThirdPartyCookieMode {
    /// CHIPS: partition only when the cookie opts in via `Partitioned`.
    Allow,
    /// Total Cookie Protection: force-partition all third-party cookies.
    #[default]
    PartitionAll,
    /// Block third-party cookies outright.
    Reject,
}

/// SameSite attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SameSite {
    #[default]
    Lax,
    Strict,
    None,
}

/// A stored cookie.
#[derive(Debug, Clone, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Domain attribute or the host the cookie came from.
    pub domain: String,
    /// True when the Domain attribute was present (domain cookie).
    pub host_only: bool,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    /// CHIPS partition key: the top-level site (schemeful eTLD+1).
    /// `None` for unpartitioned (first-party) cookies.
    pub partition_key: Option<String>,
    /// Unix seconds expiry; `None` = session cookie.
    pub expires: Option<u64>,
    pub creation_time: u64,
}

impl Cookie {
    fn is_expired(&self, now: u64) -> bool {
        self.expires.map(|e| e <= now).unwrap_or(false)
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// In-memory cookie jar (persisted snapshots handled by the engine).
#[derive(Debug, Default)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a `Set-Cookie` response header against a request URL.
    /// `top_level_site` is the registrable domain of the top-level document,
    /// used as the CHIPS partition key for `Partitioned` cookies.
    pub fn set_from_header(&mut self, url: &url::Url, header: &str, top_level_site: &str) {
        let Some(mut pc) = parse_set_cookie(header) else {
            return;
        };
        let Some(host) = url.host_str() else {
            return;
        };
        let host = host.to_ascii_lowercase();

        // Domain attribute (RFC 6265 5.2.3)
        if let Some(domain) = pc.domain_attr.clone() {
            let domain = domain.trim_start_matches('.').to_ascii_lowercase();
            // Reject cross-domain cookies (public suffix protection).
            if !host.ends_with(&domain) || registrable_domain(&host) != registrable_domain(&domain)
            {
                return;
            }
            pc.domain = domain;
            pc.host_only = false;
        } else {
            pc.domain = host;
            pc.host_only = true;
        }

        // Path attribute (default-directory algorithm, RFC 6265 5.2.4)
        match pc.path_attr.clone() {
            None => {
                let path = url.path();
                pc.path = match path.rfind('/') {
                    Some(i) if i > 0 => path[..i].to_string(),
                    _ => "/".to_string(),
                };
            }
            Some(p) => pc.path = p,
        }
        if !pc.path.starts_with('/') {
            pc.path.insert(0, '/');
        }

        pc.secure |= url.scheme() == "https";
        pc.partition_key =
            if pc.partitioned { Some(registrable_domain(top_level_site)) } else { None };
        pc.creation_time = now_secs();

        // Replace existing cookie with same identity, then append (RFC 6265 5.3.11)
        self.cookies.retain(|c| !same_identity(c, &pc));
        self.cookies.push(pc.into_cookie());
    }

    /// Policy-aware Set-Cookie processing (Phase 4 Area 2.6).
    ///
    /// * [`ThirdPartyCookieMode::Allow`] — CHIPS semantics: only cookies
    ///   carrying the `Partitioned` attribute get a partition key.
    /// * [`ThirdPartyCookieMode::PartitionAll`] — Total Cookie Protection:
    ///   every third-party cookie is force-partitioned under the top-level
    ///   site, regardless of the attribute.
    /// * [`ThirdPartyCookieMode::Reject`] — third-party cookies are
    ///   dropped entirely; first-party unaffected.
    ///
    /// `__Host-`-prefixed names additionally require Secure, no Domain
    /// attribute and Path=/ (RFC 6265bis §4.1.3.2) — violations reject the
    /// cookie in every mode.
    pub fn set_from_header_with_policy(
        &mut self,
        url: &url::Url,
        header: &str,
        top_level_site: &str,
        mode: ThirdPartyCookieMode,
    ) {
        let Some(mut pc) = parse_set_cookie(header) else {
            return;
        };
        let Some(host) = url.host_str() else {
            return;
        };
        let host = host.to_ascii_lowercase();

        // __Host- prefix requirements (checked before anything else).
        if pc.name.starts_with("__Host-") {
            let ok = pc.secure && pc.domain_attr.is_none() && pc.path_attr.as_deref() == Some("/");
            if !ok {
                return;
            }
        }

        let top_site = registrable_domain(top_level_site);
        let own_site = registrable_domain(&host);
        let third_party = !top_site.is_empty() && top_site != own_site;

        match mode {
            ThirdPartyCookieMode::Reject if third_party => return,
            ThirdPartyCookieMode::PartitionAll if third_party && pc.partition_key.is_none() => {
                // Force-partition under the top-level site (Total Cookie
                // Protection). Partitioned-with-key cookies keep their key.
                pc.partitioned = true;
            }
            _ => {}
        }

        self.set_from_header(url, header, top_level_site);
        // set_from_header recomputed partition_key from pc.partitioned via
        // its own parse; for PartitionAll we must overwrite it (the parse
        // inside does not know the policy).
        if matches!(mode, ThirdPartyCookieMode::PartitionAll) && third_party {
            let key = Some(top_site.clone());
            for c in self.cookies.iter_mut() {
                if c.name == pc.name && c.domain == host {
                    c.partition_key = key.clone();
                }
            }
        }
    }

    /// Serialize the cookie header for a request URL.
    /// `top_level_site`: CHIPS partition of the embedding document.
    /// `is_third_party`: whether the request URL is cross-site.
    pub fn header_for_url(
        &self,
        url: &url::Url,
        top_level_site: &str,
        is_third_party: bool,
    ) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let now = now_secs();
        let partition = registrable_domain(top_level_site);

        let mut matched: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| !c.is_expired(now))
            .filter(|c| domain_match(&host, c))
            .filter(|c| path_match(url.path(), &c.path))
            .filter(|c| !(c.secure && url.scheme() != "https" && url.scheme() != "wss"))
            .filter(|c| {
                // CHIPS: third-party requests only see partitioned cookies.
                if is_third_party {
                    c.partition_key.as_deref() == Some(partition.as_str())
                } else {
                    c.partition_key.is_none()
                        || c.partition_key.as_deref() == Some(partition.as_str())
                }
            })
            .collect();

        // Longer paths first, then earlier creation (RFC 6265 5.4)
        matched.sort_by(|a, b| {
            b.path.len().cmp(&a.path.len()).then(a.creation_time.cmp(&b.creation_time))
        });

        if matched.is_empty() {
            None
        } else {
            Some(
                matched
                    .iter()
                    .map(|c| format!("{}={}", c.name, c.value))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }

    /// Cookies for JavaScript `document.cookie` (HttpOnly excluded).
    pub fn js_cookies(&self, url: &url::Url) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let now = now_secs();
        let mut matched: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| !c.http_only && !c.is_expired(now))
            .filter(|c| domain_match(&host, c))
            .filter(|c| path_match(url.path(), &c.path))
            .collect();
        matched.sort_by(|a, b| {
            b.path.len().cmp(&a.path.len()).then(a.creation_time.cmp(&b.creation_time))
        });
        if matched.is_empty() {
            None
        } else {
            Some(
                matched
                    .iter()
                    .map(|c| format!("{}={}", c.name, c.value))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }

    /// Delete cookies matching name (and optionally domain substring).
    pub fn remove(&mut self, name: &str, domain_contains: Option<&str>) -> usize {
        let before = self.cookies.len();
        self.cookies.retain(|c| {
            !(c.name == name && domain_contains.map(|d| c.domain.contains(d)).unwrap_or(true))
        });
        before - self.cookies.len()
    }

    /// Drop all expired cookies; returns how many were removed.
    pub fn expire(&mut self) -> usize {
        let now = now_secs();
        let before = self.cookies.len();
        self.cookies.retain(|c| !c.is_expired(now));
        before - self.cookies.len()
    }

    /// All cookies (for persistence snapshots).
    pub fn all(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Restore from a persistence snapshot.
    pub fn restore(&mut self, cookies: Vec<Cookie>) {
        self.cookies = cookies;
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

fn same_identity(existing: &Cookie, new: &ParsedCookie) -> bool {
    existing.name == new.name
        && existing.domain == new.domain
        && existing.path == new.path
        && existing.partition_key == new.partition_key
}

/// RFC 6265 5.1.3 domain-match.
fn domain_match(request_host: &str, cookie: &Cookie) -> bool {
    if cookie.host_only {
        request_host == cookie.domain
    } else {
        request_host == cookie.domain
            || (request_host.ends_with(&cookie.domain)
                && request_host.strip_suffix(&cookie.domain).and_then(|s| s.chars().last())
                    == Some('.'))
    }
}

/// RFC 6265 5.1.4 path-match.
fn path_match(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if request_path.starts_with(cookie_path) {
        if cookie_path.ends_with('/') {
            return true;
        }
        return request_path.strip_prefix(cookie_path).is_some_and(|rest| rest.starts_with('/'));
    }
    false
}

/// Intermediate parse result.
#[derive(Debug, Clone)]
struct ParsedCookie {
    name: String,
    value: String,
    domain_attr: Option<String>,
    path_attr: Option<String>,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
    partitioned: bool,
    expires: Option<u64>,
    domain: String,
    host_only: bool,
    partition_key: Option<String>,
    creation_time: u64,
}

fn parse_set_cookie(header: &str) -> Option<ParsedCookie> {
    let mut parts = header.split(';');
    let first = parts.next()?.trim().to_string();
    let (name, value) = match first.split_once('=') {
        Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
        None => (first.clone(), String::new()),
    };
    if name.is_empty() {
        return None;
    }

    let mut pc = ParsedCookie {
        name,
        value,
        domain_attr: None,
        path_attr: None,
        path: String::new(),
        secure: false,
        http_only: false,
        same_site: SameSite::Lax,
        partitioned: false,
        expires: None,
        domain: String::new(),
        host_only: true,
        partition_key: None,
        creation_time: 0,
    };

    for attr in parts {
        let attr = attr.trim();
        let (k, v) = match attr.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
            None => (attr.to_ascii_lowercase(), String::new()),
        };
        match k.as_str() {
            "domain" => pc.domain_attr = Some(v),
            "path" => pc.path_attr = Some(v),
            "secure" => pc.secure = true,
            "httponly" => pc.http_only = true,
            "partitioned" => pc.partitioned = true,
            "samesite" => {
                pc.same_site = match v.to_ascii_lowercase().as_str() {
                    "strict" => SameSite::Strict,
                    "none" => SameSite::None,
                    _ => SameSite::Lax,
                }
            }
            "max-age" => {
                if let Ok(secs) = v.parse::<i64>() {
                    pc.expires = Some((now_secs() as i64).saturating_add(secs) as u64);
                }
            }
            "expires" => {
                if let Ok(t) = httpdate::parse_http_date(&v) {
                    pc.expires =
                        Some(t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
                }
            }
            _ => {}
        }
    }

    Some(pc)
}

impl ParsedCookie {
    fn into_cookie(self) -> Cookie {
        Cookie {
            name: self.name,
            value: self.value,
            domain: self.domain,
            host_only: self.host_only,
            path: self.path,
            secure: self.secure,
            http_only: self.http_only,
            same_site: self.same_site,
            partition_key: self.partition_key,
            expires: self.expires,
            creation_time: self.creation_time,
        }
    }
}

/// Cookie header mapping helper for tests and the network layer.
pub type CookieMap = HashMap<String, String>;

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn basic_set_and_send() {
        let mut jar = CookieJar::new();
        let u = url("https://example.com/");
        jar.set_from_header(&u, "sid=abc123; Path=/; HttpOnly", "example.com");
        let header = jar.header_for_url(&u, "example.com", false).unwrap();
        assert_eq!(header, "sid=abc123");
        assert!(jar.js_cookies(&u).is_none(), "HttpOnly must be hidden from JS");
    }

    #[test]
    fn path_matching() {
        let mut jar = CookieJar::new();
        let deep = url("https://example.com/a/b/index.html");
        jar.set_from_header(&deep, "deep=1", "example.com");
        let root = url("https://example.com/");
        assert!(jar.header_for_url(&root, "example.com", false).is_none());
        assert!(jar.header_for_url(&deep, "example.com", false).is_some());
    }

    #[test]
    fn domain_cookie_rejects_cross_site() {
        let mut jar = CookieJar::new();
        let u = url("https://attacker.example.com/");
        jar.set_from_header(&u, "x=1; Domain=other.com", "attacker.example.com");
        assert_eq!(jar.len(), 0);
        jar.set_from_header(&u, "y=1; Domain=example.com", "attacker.example.com");
        assert_eq!(jar.len(), 1);
        assert!(!jar.all()[0].host_only);
    }

    #[test]
    fn chips_partitioning() {
        let mut jar = CookieJar::new();
        let u = url("https://cdn.thirdparty.com/");
        // Embedded by site a.com → partition key a.com
        jar.set_from_header(&u, "chips=1; Partitioned; Secure", "a.com");
        let c = &jar.all()[0];
        assert_eq!(c.partition_key.as_deref(), Some("a.com"));
        // Third-party request embedded by b.com must NOT see it.
        assert!(jar.header_for_url(&u, "b.com", true).is_none());
        // Embedded by a.com → visible.
        assert!(jar.header_for_url(&u, "a.com", true).is_some());
    }

    #[test]
    fn max_age_expiry() {
        let mut jar = CookieJar::new();
        let u = url("https://example.com/");
        jar.set_from_header(&u, "gone=1; Max-Age=0", "example.com");
        assert!(jar.header_for_url(&u, "example.com", false).is_none());
        jar.set_from_header(&u, "stays=1; Max-Age=3600", "example.com");
        assert_eq!(jar.expire(), 1); // garbage-collect the expired entry
        assert_eq!(jar.len(), 1);
    }

    #[test]
    fn registrable_domain_psl() {
        assert_eq!(registrable_domain("a.b.example.co.uk"), "example.co.uk");
        assert_eq!(registrable_domain("example.com"), "example.com");
        assert_eq!(registrable_domain("localhost"), "localhost");
        assert_eq!(registrable_domain("192.168.1.1"), "192.168.1.1");
    }

    #[test]
    fn secure_cookies_not_sent_over_http() {
        let mut jar = CookieJar::new();
        let https = url("https://example.com/");
        jar.set_from_header(&https, "s=1; Secure", "example.com");
        let http = url("http://example.com/");
        assert!(jar.header_for_url(&http, "example.com", false).is_none());
        assert!(jar.header_for_url(&https, "example.com", false).is_some());
    }
}

#[cfg(test)]
mod tcp_tests {
    use super::*;

    fn url(scheme: &str, host: &str) -> url::Url {
        url::Url::parse(&format!("{scheme}://{host}/page/index.html")).unwrap()
    }

    #[test]
    fn total_protection_force_partitions_third_party() {
        let mut jar = CookieJar::new();
        // tracker.com sets a plain (unpartitioned) cookie inside site.com.
        let u = url("https", "tracker.com");
        jar.set_from_header_with_policy(
            &u,
            "sid=abc123; Path=/",
            "site.com",
            ThirdPartyCookieMode::PartitionAll,
        );
        let all = jar.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].partition_key.as_deref(), Some("site.com"));
    }

    #[test]
    fn total_protection_leaves_first_party_unpartitioned() {
        let mut jar = CookieJar::new();
        let u = url("https", "site.com");
        jar.set_from_header_with_policy(
            &u,
            "session=xyz; Path=/",
            "site.com",
            ThirdPartyCookieMode::PartitionAll,
        );
        let all = jar.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].partition_key, None);
    }

    #[test]
    fn allow_mode_partitions_only_explicit_chips() {
        let mut jar = CookieJar::new();
        let u = url("https", "tracker.com");
        jar.set_from_header_with_policy(
            &u,
            "plain=1; Path=/",
            "site.com",
            ThirdPartyCookieMode::Allow,
        );
        assert_eq!(jar.all()[0].partition_key, None);
        jar.set_from_header_with_policy(
            &u,
            "chips=1; Path=/; Secure; Partitioned",
            "site.com",
            ThirdPartyCookieMode::Allow,
        );
        assert_eq!(jar.all()[1].partition_key.as_deref(), Some("site.com"));
    }

    #[test]
    fn reject_mode_drops_third_party_keeps_first_party() {
        let mut jar = CookieJar::new();
        jar.set_from_header_with_policy(
            &url("https", "tracker.com"),
            "t=1; Path=/",
            "site.com",
            ThirdPartyCookieMode::Reject,
        );
        jar.set_from_header_with_policy(
            &url("https", "site.com"),
            "f=1; Path=/",
            "site.com",
            ThirdPartyCookieMode::Reject,
        );
        assert_eq!(jar.len(), 1);
        assert_eq!(jar.all()[0].name, "f");
    }

    #[test]
    fn host_prefix_rules_enforced() {
        let mut jar = CookieJar::new();
        // Valid __Host- cookie.
        jar.set_from_header_with_policy(
            &url("https", "site.com"),
            "__Host-a=1; Path=/; Secure",
            "site.com",
            ThirdPartyCookieMode::PartitionAll,
        );
        // Missing Secure -> rejected.
        jar.set_from_header_with_policy(
            &url("https", "site.com"),
            "__Host-b=1; Path=/",
            "site.com",
            ThirdPartyCookieMode::PartitionAll,
        );
        // Domain attribute present -> rejected.
        jar.set_from_header_with_policy(
            &url("https", "site.com"),
            "__Host-c=1; Path=/; Secure; Domain=site.com",
            "site.com",
            ThirdPartyCookieMode::PartitionAll,
        );
        // Path != / -> rejected.
        jar.set_from_header_with_policy(
            &url("https", "site.com"),
            "__Host-d=1; Path=/sub; Secure",
            "site.com",
            ThirdPartyCookieMode::PartitionAll,
        );
        let names: Vec<&str> = jar.all().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["__Host-a"]);
    }
}
