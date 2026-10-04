//! Phase 4 Area 4.5 — tab search: a fast, in-memory index over the
//! open tabs (title, URL, content snippet) exposed to the shell (FIFO
//! now, UI palette later).
//!
//! Pure data + policy (no engine types), unit-testable. The shell
//! upserts entries as tab metadata changes (title/URL from its own
//! bookkeeping; the snippet comes from a page-description evaluation
//! stored in `HostState::page_snippet`).

use std::collections::HashMap;

/// One indexed tab.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchEntry {
    pub tab_id: u64,
    pub title: String,
    pub url: String,
    /// Short page-content summary (meta description / first heading).
    pub snippet: Option<String>,
}

/// Field weights: titles are the strongest signal, URLs next, content
/// snippets last (they can be boilerplate-heavy).
pub const W_TITLE: f32 = 3.0;
pub const W_URL: f32 = 2.0;
pub const W_SNIPPET: f32 = 1.0;
/// Bonus when the field STARTS with the query (title bar exactness).
pub const W_PREFIX_BONUS: f32 = 0.5;

#[derive(Debug, Clone, Default)]
pub struct TabSearchIndex {
    entries: HashMap<u64, SearchEntry>,
}

impl TabSearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert or refresh a tab's entry (idempotent per tab).
    pub fn upsert(&mut self, tab_id: u64, title: &str, url: &str, snippet: Option<&str>) {
        self.entries.insert(
            tab_id,
            SearchEntry {
                tab_id,
                title: title.to_string(),
                url: url.to_string(),
                snippet: snippet.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            },
        );
    }

    pub fn remove(&mut self, tab_id: u64) {
        self.entries.remove(&tab_id);
    }

    pub fn entry(&self, tab_id: u64) -> Option<&SearchEntry> {
        self.entries.get(&tab_id)
    }

    /// Search: whitespace-separated tokens, ALL must match somewhere
    /// (AND) case-insensitively; score is the sum of matched field
    /// weights (+ prefix bonus). Returns `(tab_id, score)` best-first,
    /// capped at `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(u64, f32)> {
        let tokens: Vec<String> =
            query.split_whitespace().map(|t| t.to_lowercase()).filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u64, f32)> = self
            .entries
            .values()
            .filter_map(|e| {
                let title = e.title.to_lowercase();
                let url = e.url.to_lowercase();
                let snippet = e.snippet.as_deref().unwrap_or("").to_lowercase();
                // Every token must hit at least one field.
                let mut total = 0.0f32;
                for tok in &tokens {
                    let mut best = 0.0f32;
                    if let Some(pos) = title.find(tok) {
                        best =
                            best.max(W_TITLE + (pos == 0).then_some(W_PREFIX_BONUS).unwrap_or(0.0));
                    }
                    if let Some(pos) = url.find(tok) {
                        best =
                            best.max(W_URL + (pos == 0).then_some(W_PREFIX_BONUS).unwrap_or(0.0));
                    }
                    if snippet.contains(tok) {
                        best = best.max(W_SNIPPET);
                    }
                    if best == 0.0 {
                        return None; // AND semantics violated
                    }
                    total += best;
                }
                Some((e.tab_id, total))
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> TabSearchIndex {
        let mut i = TabSearchIndex::new();
        i.upsert(
            1,
            "Rust Programming Language",
            "https://www.rust-lang.org/",
            Some("A language empowering everyone"),
        );
        i.upsert(
            2,
            "GitHub - servo/servo",
            "https://github.com/servo/servo",
            Some("The Servo browser engine"),
        );
        i.upsert(3, "Hacker News", "https://news.ycombinator.com/", None);
        i
    }

    #[test]
    fn title_beats_url_beats_snippet() {
        let i = idx();
        // "rust" is in a title AND a url AND a snippet; the title match ranks first.
        let hits = i.search("rust", 10);
        assert_eq!(hits[0].0, 1, "title match first: {hits:?}");
        // "engine" only appears in a snippet.
        let hits = i.search("engine", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 2);
    }

    #[test]
    fn and_semantics() {
        let i = idx();
        // "servo github": both tokens only co-occur in tab 2.
        let hits = i.search("servo github", 10);
        assert_eq!(hits, vec![(2, hits[0].1)]);
        // "rust news": no single entry has both → no results.
        assert!(i.search("rust news", 10).is_empty());
    }

    #[test]
    fn prefix_bonus_orders_results() {
        let mut i = TabSearchIndex::new();
        i.upsert(1, "The Rust Book", "https://doc.rust-lang.org/book/", None);
        i.upsert(2, "Rust Blog", "https://blog.rust-lang.org/", None);
        // Both titles contain "rust"; tab 2 STARTS with it.
        let hits = i.search("rust", 10);
        assert_eq!(hits[0].0, 2, "prefix match ranks first: {hits:?}");
    }

    #[test]
    fn upsert_remove_and_case() {
        let mut i = TabSearchIndex::new();
        i.upsert(5, "Example", "https://example.com/", None);
        assert_eq!(i.len(), 1);
        // Re-upsert refreshes, does not duplicate.
        i.upsert(5, "Example Domain", "https://example.com/", Some("illustrative examples"));
        assert_eq!(i.len(), 1);
        assert_eq!(i.entry(5).unwrap().title, "Example Domain");
        // Case-insensitive match.
        assert_eq!(i.search("EXAMPLE", 10).len(), 1);
        assert_eq!(i.search("domain", 10).len(), 1);
        i.remove(5);
        assert!(i.is_empty());
        assert!(i.search("example", 10).is_empty());
    }

    #[test]
    fn empty_and_blank_queries() {
        let i = idx();
        assert!(i.search("", 10).is_empty());
        assert!(i.search("   ", 10).is_empty());
    }
}
