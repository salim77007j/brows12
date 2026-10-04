//! Phase 4 Area 4.1 — predictive hibernation policy (lightweight
//! heuristics, no in-process ML by design).
//!
//! Division of labour (same split as `memory.rs` / `budget.rs`):
//! - this module: pure policy over plain numbers (unit-testable);
//! - the shell (`ui`): records activations, builds the per-tab `TabUsage`
//!   vector aligned with the tab strip, and executes the hibernation
//!   order produced here.
//!
//! Model: every tab carries usage signals (activation count, time since
//! the user last had it active, page weight). `return_score` condenses
//! them into a 0.0..=1.0 "how likely is the user to come back to this
//! tab soon" estimate; `hibernation_order` ranks candidates so the tab
//! the user is LEAST likely to return to is suspended first, with the
//! heaviest pages first among near-equal scores (they buy back the most
//! memory per suspension, which is the point of the reclaim).

/// Half-life of recency: a tab last used this long ago scores half of a
/// just-used tab. 30 min matches the default idle-suspend horizon
/// (`BROWS12_TAB_SUSPEND_SECS` = 180 s is the *eligibility* floor; the
/// half-life shapes the ranking above it).
pub const RECENCY_HALF_LIFE_MS: u64 = 30 * 60 * 1000;

/// Weight of each predictive signal in `return_score` (sum to 1.0).
/// Page weight is deliberately NOT a predictor — it enters the ranking
/// as a cost (tie-break + the heavy-eligibility schedule, which stay
/// governor-side), so a heavy page the user just left is not
/// suspended before a light page they abandoned hours ago.
pub const W_RECENCY: f64 = 0.65;
pub const W_FREQUENCY: f64 = 0.35;

/// Usage signals for one tab, aligned by index with the shell's tab
/// strip. `page_requests` doubles as the weight proxy (same signal the
/// Phase 2 governor uses).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabUsage {
    /// How many times this tab became the active tab (predictive signal:
    /// often-used tabs come back).
    pub activations: u32,
    /// Monotonic-ish ms timestamp of the last activation (0 = never).
    pub last_active_ms: u64,
    /// Page weight proxy: resource requests issued by the document.
    pub page_requests: u64,
}

impl TabUsage {
    pub fn record_activation(&mut self, now_ms: u64) {
        self.activations = self.activations.saturating_add(1);
        self.last_active_ms = now_ms.max(self.last_active_ms);
    }

    /// ms since the last activation (u64::MAX for never-activated tabs
    /// so they sort as infinitely stale).
    pub fn since_active_ms(&self, now_ms: u64) -> u64 {
        if self.last_active_ms == 0 {
            return u64::MAX;
        }
        now_ms.saturating_sub(self.last_active_ms)
    }
}

/// Exponential recency factor in 0.0..=1.0: 1.0 just used, 0.5 at the
/// half-life, decaying to ~0 for very stale tabs. Never-activated tabs
/// (since = u64::MAX) decay to exactly 0.
pub fn recency_factor(since_active_ms: u64) -> f64 {
    if since_active_ms == u64::MAX || since_active_ms == 0 {
        return if since_active_ms == 0 { 1.0 } else { 0.0 };
    }
    let halves = since_active_ms as f64 / RECENCY_HALF_LIFE_MS as f64;
    0.5_f64.powf(halves)
}

/// Frequency factor in 0.0..=1.0 with diminishing returns:
/// a/(1+a) — 1 activation → 0.5, 3 → 0.75, 9 → 0.9.
pub fn frequency_factor(activations: u32) -> f64 {
    let a = activations as f64;
    a / (1.0 + a)
}

/// Weight factor in 0.0..=1.0, normalized against the heaviest tab in
/// the strip (relative weight: the score is comparative by design —
/// prediction only has to RANK tabs, not match an absolute scale).
pub fn weight_factor(page_requests: u64, max_requests_in_strip: u64) -> f64 {
    if max_requests_in_strip == 0 {
        return 0.0;
    }
    (page_requests as f64 / max_requests_in_strip as f64).min(1.0)
}

/// Return-probability score in 0.0..=1.0 (higher = more likely the user
/// comes back to this tab soon):
/// `0.65·recency + 0.35·frequency`.
///
/// Page weight is intentionally excluded from the prediction (see the
/// `W_RECENCY` note); it is a COST applied by `hibernation_order`'s
/// tie-break and by the governor's existing heavy-tab schedule.
pub fn return_score(usage: &TabUsage, now_ms: u64, _max_requests_in_strip: u64) -> f64 {
    W_RECENCY * recency_factor(usage.since_active_ms(now_ms))
        + W_FREQUENCY * frequency_factor(usage.activations)
}

/// Rank hibernation candidates: LEAST likely to be returned to first.
/// Ties (scores within `TIE_EPS`) break by heavier page first — when
/// prediction is indifferent, suspending the heaviest buys back the
/// most memory. Returns the candidate indices in suspension order.
pub fn hibernation_order(usages: &[TabUsage], candidates: &[usize], now_ms: u64) -> Vec<usize> {
    const TIE_EPS: f64 = 0.02;
    let max_req = usages.iter().map(|u| u.page_requests).max().unwrap_or(0);
    let mut scored: Vec<(usize, f64)> = candidates
        .iter()
        .filter(|&&i| i < usages.len())
        .map(|&i| (i, return_score(&usages[i], now_ms, max_req)))
        .collect();
    scored.sort_by(|a, b| {
        let (sa, sb) = (a.1, b.1);
        if (sa - sb).abs() < TIE_EPS {
            // Near-equal prediction: heavier first, then staler first.
            let wa = usages[a.0].page_requests;
            let wb = usages[b.0].page_requests;
            wb.cmp(&wa)
                .then(usages[a.0].since_active_ms(now_ms).cmp(&usages[b.0].since_active_ms(now_ms)))
        } else {
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        // Large enough that "60 minutes ago" stays positive.
        100_000_000
    }

    #[test]
    fn activation_recording() {
        let mut u = TabUsage::default();
        assert_eq!(u.activations, 0);
        assert_eq!(u.since_active_ms(now()), u64::MAX);
        u.record_activation(now() - 1_000);
        assert_eq!(u.activations, 1);
        assert_eq!(u.since_active_ms(now()), 1_000);
        // Stale timestamps never move the clock backwards.
        u.record_activation(now() - 2_000);
        assert_eq!(u.last_active_ms, now() - 1_000);
        assert_eq!(u.activations, 2);
    }

    #[test]
    fn recency_decays_by_half_life() {
        assert!((recency_factor(0) - 1.0).abs() < 1e-9);
        let half = recency_factor(RECENCY_HALF_LIFE_MS);
        assert!((half - 0.5).abs() < 1e-9);
        let two = recency_factor(2 * RECENCY_HALF_LIFE_MS);
        assert!((two - 0.25).abs() < 1e-9);
        // Never-activated is maximally stale.
        assert_eq!(recency_factor(u64::MAX), 0.0);
    }

    #[test]
    fn frequency_has_diminishing_returns() {
        assert_eq!(frequency_factor(0), 0.0);
        assert!((frequency_factor(1) - 0.5).abs() < 1e-9);
        assert!(frequency_factor(10) > frequency_factor(3));
        assert!(frequency_factor(1000) < 1.0);
    }

    #[test]
    fn score_orders_by_likelihood() {
        let t = now();
        // Just-activated, frequently used tab.
        let hot = TabUsage { activations: 10, last_active_ms: t - 1_000, page_requests: 40 };
        // Stale, never re-visited tab.
        let cold =
            TabUsage { activations: 1, last_active_ms: t - 60 * 60 * 1000, page_requests: 5 };
        assert!(return_score(&hot, t, 40) > return_score(&cold, t, 40));
        // Score stays in range for both extremes.
        for u in [&hot, &cold] {
            let s = return_score(u, t, 40);
            assert!((0.0..=1.0).contains(&s), "score {s} out of range");
        }
    }

    #[test]
    fn hibernation_order_least_likely_first() {
        let t = now();
        let usages = vec![
            TabUsage { activations: 1, last_active_ms: t - 55 * 60 * 1000, page_requests: 10 }, // 0: stale
            TabUsage { activations: 9, last_active_ms: t - 2 * 60 * 1000, page_requests: 90 }, // 1: hot + heavy
            TabUsage { activations: 2, last_active_ms: t - 50 * 60 * 1000, page_requests: 30 }, // 2: stale-ish
            TabUsage { activations: 5, last_active_ms: t - 5 * 60 * 1000, page_requests: 15 }, // 3: warm
        ];
        let order = hibernation_order(&usages, &[0, 1, 2, 3], t);
        // The never-coming-back tab goes first; the hot tab last.
        assert_eq!(order[0], 0, "stale tab hibernates first, got {order:?}");
        assert_eq!(*order.last().unwrap(), 1, "hot tab hibernates last, got {order:?}");
    }

    #[test]
    fn tie_breaks_by_weight() {
        let t = now();
        // Same recency, same activations — heavier tab first.
        let usages = vec![
            TabUsage { activations: 2, last_active_ms: t - 10 * 60 * 1000, page_requests: 5 },
            TabUsage { activations: 2, last_active_ms: t - 10 * 60 * 1000, page_requests: 500 },
        ];
        let order = hibernation_order(&usages, &[0, 1], t);
        assert_eq!(order, vec![1, 0], "heavier tab first on a tie, got {order:?}");
    }

    #[test]
    fn out_of_range_candidates_are_ignored() {
        let t = now();
        let usages = vec![TabUsage { activations: 1, last_active_ms: t, page_requests: 1 }];
        let order = hibernation_order(&usages, &[0, 5, 9], t);
        assert_eq!(order, vec![0]);
    }
}
