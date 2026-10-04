//! `$removeparam` matching (tracking query-parameter stripping).
//!
//! The upstream `adblock` 0.13.3 engine parses `$removeparam` rules but
//! never matches them (verified by minimal repro: `||example.com^
//! $removeparam=utm_source` returns `rewritten_url = None`; filed
//! upstream). Until that is fixed we implement the uBO-compatible subset
//! ourselves for the rules brows12 ships:
//!
//! ```text
//! *$removeparam=NAME              — strip NAME from every request
//! $removeparam=NAME               — same
//! ||HOST^$removeparam=NAME        — only on requests to HOST (or subdomains)
//! …,domain=A|~B                   — only when the source site matches A, not B
//! ```
//!
//! Unsupported (skipped, counted): regex values `/…/`, negated values
//! `~NAME`, value constraints `NAME=value`, request-type options
//! (`$xhr`, `$image`, …), `$important`, `$tag`.

/// One parsed `$removeparam` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveparamRule {
    /// `None` = any host (`*`); `Some(host)` = `||host^` anchored.
    pub host: Option<String>,
    /// Parameter name to strip (lowercased).
    pub param: String,
    /// Source-site domain constraints (`domain=` option, `!` = negated).
    pub domains: Vec<(String, bool)>,
}

/// Parse the handled subset from one filter line.
pub fn parse_rule(line: &str) -> Option<RemoveparamRule> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('!') || line.starts_with("@@") {
        return None;
    }
    let (pattern_part, options_part) = line.split_once('$')?;
    let mut removeparam: Option<&str> = None;
    let mut domains = Vec::new();
    for opt in options_part.split(',') {
        if let Some(rp) = opt.strip_prefix("removeparam=") {
            removeparam = Some(rp);
        } else if let Some(d) = opt.strip_prefix("domain=") {
            for entry in d.split('|') {
                match entry.strip_prefix('~') {
                    Some(neg) => domains.push((neg.to_ascii_lowercase(), false)),
                    None => domains.push((entry.to_ascii_lowercase(), true)),
                }
            }
        } else if opt.is_empty() {
            // trailing comma
        } else {
            // Any other option ($xhr, $image, $important, $tag, …) puts the
            // rule outside our subset.
            return None;
        }
    }
    let removeparam = removeparam?;

    // Value forms we do not support.
    if removeparam.starts_with('/')
        || removeparam.starts_with('~')
        || removeparam.contains('=')
        || removeparam.is_empty()
        || !removeparam.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }

    let pattern = pattern_part.trim();
    let host = match pattern {
        // any host
        "*" | "" => None,
        p => {
            let Some(h) = p.strip_prefix("||") else {
                // Arbitrary URL patterns are outside the subset.
                return None;
            };
            let h = h.strip_suffix('^').unwrap_or(h);
            if h.is_empty() || h.contains('*') {
                return None;
            }
            Some(h.to_ascii_lowercase())
        }
    };

    Some(RemoveparamRule { host, param: removeparam.to_ascii_lowercase(), domains })
}

/// Parse a whole list (skips comments/metadata).
pub fn parse_list(text: &str) -> Vec<RemoveparamRule> {
    text.lines().filter_map(parse_rule).collect()
}

fn host_matches(rule_host: &str, request_host: &str) -> bool {
    request_host == rule_host || request_host.ends_with(&format!(".{rule_host}"))
}

fn domain_option_matches(domains: &[(String, bool)], source_site: &str) -> bool {
    if domains.is_empty() {
        return true;
    }
    let mut any_positive = false;
    let mut matched_positive = false;
    for (domain, positive) in domains {
        if *positive {
            any_positive = true;
            if source_site == domain || source_site.ends_with(&format!(".{domain}")) {
                matched_positive = true;
            }
        } else if source_site == domain || source_site.ends_with(&format!(".{domain}")) {
            return false;
        }
    }
    !any_positive || matched_positive
}

/// Strip tracking parameters from `url`.
///
/// * `request_host` — host of the request URL.
/// * `source_site`  — registrable domain of the embedding document
///   (empty string when unknown; `domain=` rules then cannot match).
///
/// Returns the rewritten URL when at least one parameter was removed.
pub fn strip_params(
    rules: &[RemoveparamRule],
    url: &str,
    request_host: &str,
    source_site: &str,
) -> Option<String> {
    let (before_query, query_and_hash) = url.split_once('?')?;
    if query_and_hash.is_empty() {
        return None;
    }
    let (query, hash) = match query_and_hash.split_once('#') {
        Some((q, h)) => (q, Some(h)),
        None => (query_and_hash, None),
    };

    let applicable: Vec<&RemoveparamRule> = rules
        .iter()
        .filter(|r| {
            r.host.as_ref().map(|h| host_matches(h, request_host)).unwrap_or(true)
                && domain_option_matches(&r.domains, source_site)
        })
        .collect();
    if applicable.is_empty() {
        return None;
    }

    let mut removed = false;
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            if pair.is_empty() {
                return true;
            }
            let key = pair.split('=').next().unwrap_or(pair).to_ascii_lowercase();
            let hit = applicable.iter().any(|r| r.param == key);
            if hit {
                removed = true;
            }
            !hit
        })
        .collect();

    if !removed {
        return None;
    }
    let mut out = String::with_capacity(url.len());
    out.push_str(before_query);
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(h) = hash {
        out.push('#');
        out.push_str(h);
    }
    Some(out)
}

/// The built-in rule set: `privacy/lists/brows12_extras.txt` `$removeparam`
/// lines, parsed at startup.
pub fn default_rules() -> Vec<RemoveparamRule> {
    parse_list(crate::blocker::BROWS12_EXTRAS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_rules() {
        let r = parse_rule("*$removeparam=utm_source").unwrap();
        assert_eq!(r.param, "utm_source");
        assert_eq!(r.host, None);
        let r = parse_rule("||fastlane.rubiconproject.com^$removeparam,domain=aternos.org");
        assert!(r.is_none(), "valueless removeparam is outside the subset");
        let r = parse_rule(
            "||clips-manifests-aka.warnermediacdn.com^$xhr,removeparam=caid,domain=nba.com",
        );
        assert!(r.is_none(), "type-restricted rules are outside the subset");
    }

    #[test]
    fn strips_utm_everywhere() {
        let rules = parse_list("*$removeparam=utm_source\n*$removeparam=fbclid\n");
        let out = strip_params(
            &rules,
            "https://example.com/get?utm_source=brows12test&id=42",
            "example.com",
            "example.com",
        )
        .unwrap();
        assert_eq!(out, "https://example.com/get?id=42");
        // No matching params → unchanged.
        assert!(strip_params(&rules, "https://example.com/?id=1", "example.com", "example.com")
            .is_none());
        // Lone param removed entirely (no dangling ?).
        let out =
            strip_params(&rules, "https://example.com/?fbclid=x", "example.com", "example.com")
                .unwrap();
        assert_eq!(out, "https://example.com/");
    }

    #[test]
    fn host_anchored_rules() {
        let rules = parse_list("||track.example.com^$removeparam=click_id\n");
        assert!(strip_params(
            &rules,
            "https://track.example.com/a?click_id=1",
            "track.example.com",
            "example.com"
        )
        .is_some());
        assert!(strip_params(
            &rules,
            "https://sub.track.example.com/a?click_id=1",
            "sub.track.example.com",
            "example.com"
        )
        .is_some());
        assert!(strip_params(
            &rules,
            "https://other.example.com/a?click_id=1",
            "other.example.com",
            "example.com"
        )
        .is_none());
    }

    #[test]
    fn domain_option_rules() {
        let rules = parse_list("*$removeparam=foo,domain=example.com|~sub.example.com\n");
        assert!(strip_params(&rules, "https://x.com/?foo=1", "x.com", "example.com").is_some());
        assert!(strip_params(&rules, "https://x.com/?foo=1", "x.com", "www.example.com").is_some());
        assert!(strip_params(&rules, "https://x.com/?foo=1", "x.com", "sub.example.com").is_none());
        assert!(strip_params(&rules, "https://x.com/?foo=1", "x.com", "other.com").is_none());
    }

    #[test]
    fn hash_and_preserved_params_survive() {
        let rules = parse_list("*$removeparam=utm_medium\n");
        let out =
            strip_params(&rules, "https://e.com/p?keep=1&utm_medium=cpc#frag", "e.com", "e.com")
                .unwrap();
        assert_eq!(out, "https://e.com/p?keep=1#frag");
    }

    #[test]
    fn extras_list_parses() {
        let rules = default_rules();
        assert!(rules.len() >= 30, "brows12 extras carry a full tracking-param set");
        assert!(rules.iter().any(|r| r.param == "gclid"));
        assert!(rules.iter().all(|r| r.host.is_none()));
    }
}
