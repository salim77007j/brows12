//! Phase 4.3.1 — per-tab memory budgets and the graceful-degradation
//! ladder, plus the Phase 4.3.4 JS-heap tier policy.
//!
//! Division of labour (same split as `memory.rs`):
//! - this module: pure policy over plain numbers (no engine types,
//!   unit-testable);
//! - the shell (`ui`): samples RSS / MemAvailable / PSI, executes the
//!   degradation ladder (trim caches → hibernate) and applies the
//!   JS-heap tier via `Servo::set_preference`.
//!
//! Why a *per-tab* budget on top of the global RSS budget: the Phase 2
//! governor only acts on the process total, so one runaway tab can sit
//! under the global budget while starving everything else, and a light
//! tab can be hibernated before a heavier one. A per-tab budget makes
//! the reclaim decision per-tab and weight-aware, and lets the total
//! budget adapt to how much memory the *system* actually has free.

use std::time::Duration;

/// Policy knobs. Environment-tunable so CI can exercise every path.
#[derive(Debug, Clone)]
pub struct BudgetConfig {
    /// Nominal total budget for the whole browser (the Phase 2 value).
    /// Env: `BROWS12_MEM_BUDGET_MB` (shared with the governor).
    pub total_budget_kb: u64,
    /// Under system memory pressure the total budget shrinks to
    /// `baseline + availability * fraction`. Env: `BROWS12_BUDGET_AVAIL_FRACTION`
    /// (default 0.25; 0 disables availability adaption).
    pub availability_fraction: f64,
    /// Baseline kept when availability adaption shrinks the budget —
    /// the browser's own idle floor. Env: `BROWS12_BUDGET_BASELINE_MB`
    /// (default 96).
    pub baseline_kb: u64,
    /// Estimated memory of a tab that never loaded anything — every
    /// live tab costs at least this (pipeline + JS runtime floor).
    /// Env: `BROWS12_TAB_BASE_MB` (default 8).
    pub tab_base_kb: u64,
    /// Memory per resource request (DOM nodes, decoded images, caches
    /// grow with page weight; requests are the Phase 2/3 proxy).
    /// Env: `BROWS12_TAB_PER_REQUEST_KB` (default 256).
    pub per_request_kb: u64,
    /// Multiplier on the fair share for the active tab (the tab the
    /// user is looking at must not degrade while others are over).
    /// Env: `BROWS12_ACTIVE_SHARE` (default 3.0).
    pub active_share: f64,
    /// A background tab over its budget gets its caches trimmed first
    /// (hide + throttle + `malloc_trim`); if it is *still* over budget
    /// after this long, it is hibernated. Env: `BROWS12_TRIM_GRACE_SECS`
    /// (default 30).
    pub trim_grace: Duration,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            total_budget_kb: 384 * 1024,
            availability_fraction: 0.25,
            baseline_kb: 96 * 1024,
            tab_base_kb: 8 * 1024,
            per_request_kb: 256,
            active_share: 3.0,
            trim_grace: Duration::from_secs(30),
        }
    }
}

/// Estimated footprint of one tab, KiB. This is a *weight estimate*
/// (the same request-count proxy the governor uses), not a measurement:
/// per-tab engine accounting is not exposed by Servo 0.6.0 (documented
/// gap, see PHASE4_AREA3_REPORT.md §3.1). The estimate only has to rank
/// tabs against each other and against their budget — it does that.
pub fn tab_estimate_kb(page_requests: u64, cfg: &BudgetConfig) -> u64 {
    cfg.tab_base_kb.saturating_add(page_requests.saturating_mul(cfg.per_request_kb))
}

/// Total budget this session, adapted to system availability:
/// `min(total_budget, baseline + available * fraction)`.
pub fn total_budget_kb(available_kb: Option<u64>, cfg: &BudgetConfig) -> u64 {
    match available_kb {
        Some(avail) if cfg.availability_fraction > 0.0 => {
            let adaptive = cfg.baseline_kb + (avail as f64 * cfg.availability_fraction) as u64;
            cfg.total_budget_kb.min(adaptive)
        }
        _ => cfg.total_budget_kb,
    }
}

/// Budget for one tab: the active tab gets `active_share ×` the fair
/// share of the *remaining* budget, background tabs split the rest by
/// weight (heavier pages get proportionally more, but never more than
/// they are estimated to need + 25% headroom).
pub fn tab_budget_kb(
    total_kb: u64,
    is_active: bool,
    weights_kb: &[u64],
    my_index: usize,
    cfg: &BudgetConfig,
) -> u64 {
    if weights_kb.is_empty() || my_index >= weights_kb.len() {
        return cfg.tab_base_kb;
    }
    let total_weight: u64 = weights_kb.iter().sum();
    // Fair share of the whole budget for this tab, then the active-tab
    // multiplier applied by taking from the common pool.
    let share = if is_active {
        let others: u64 =
            weights_kb.iter().enumerate().filter(|(i, _)| *i != my_index).map(|(_, w)| *w).sum();
        // Active takes its weight + share multiplier out of what is left
        // after the others' weights, bounded below by its own weight.
        let pool_after_others = total_kb.saturating_sub(others);
        ((pool_after_others as f64 / cfg.active_share) as u64).max(weights_kb[my_index])
    } else {
        (total_kb as f64 * (weights_kb[my_index] as f64 / total_weight.max(1) as f64)) as u64
    };
    share.max(cfg.tab_base_kb)
}

/// One step of the graceful-degradation ladder for a background tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Degradation {
    /// Within budget — do nothing.
    None,
    /// Over budget, caches not yet trimmed: hide + throttle the tab and
    /// `malloc_trim` (frees allocator-retained arenas; for a hidden tab
    /// WebRender stops producing new frames so decoded-frame buffers
    /// age out of the caches naturally).
    TrimCaches,
    /// Over budget and the trim grace period has elapsed — drop the
    /// pipeline (hibernate; the shell's existing mechanism).
    Hibernate,
}

/// Decide the ladder step for a background tab.
///
/// `trimmed_since_ms`: `Some(ms)` timestamp of the last TrimCaches (or
/// None if never trimmed); `now_ms` the current monotonic-ish ms clock.
pub fn decide(
    estimate_kb: u64,
    budget_kb: u64,
    trimmed_since_ms: Option<u64>,
    now_ms: u64,
    cfg: &BudgetConfig,
) -> Degradation {
    if estimate_kb <= budget_kb {
        return Degradation::None;
    }
    match trimmed_since_ms {
        None => Degradation::TrimCaches,
        Some(since) => {
            let elapsed = now_ms.saturating_sub(since);
            if elapsed >= cfg.trim_grace.as_millis() as u64 {
                Degradation::Hibernate
            } else {
                Degradation::None // grace period running
            }
        }
    }
}

/// Phase 4.3.4 — JS-heap tiers. `js_mem_max` is read once per JS
/// runtime creation (servo-script 0.6.0 snapshot; runtime re-apply is
/// an upstream gap), so the tier is applied globally and takes effect
/// for every runtime created afterwards — in practice every tab
/// restored from hibernation, which is exactly when the governor wants
/// a smaller heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JsHeapTier {
    /// Default (prefs.rs): 256 MB.
    Normal,
    /// Elevated pressure: 192 MB.
    Tight,
    /// Critical pressure: 128 MB.
    Small,
    /// Critical PSI + over-budget: 96 MB.
    Minimal,
}

impl JsHeapTier {
    pub fn mem_max_mb(self) -> u64 {
        match self {
            JsHeapTier::Normal => 256,
            JsHeapTier::Tight => 192,
            JsHeapTier::Small => 128,
            JsHeapTier::Minimal => 96,
        }
    }

    /// The tier for the current conditions. `psi_hot`: PSI says the
    /// *system* is thrashing (independent of our own RSS).
    pub fn for_conditions(rss_ratio: f64, psi_hot: bool, over_budget_tabs: usize) -> JsHeapTier {
        if psi_hot && rss_ratio >= 0.8 {
            JsHeapTier::Minimal
        } else if psi_hot || rss_ratio >= 1.0 {
            JsHeapTier::Small
        } else if rss_ratio >= 0.8 || over_budget_tabs > 0 {
            JsHeapTier::Tight
        } else {
            JsHeapTier::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_estimate_scales_with_requests() {
        let cfg = BudgetConfig::default();
        assert_eq!(tab_estimate_kb(0, &cfg), 8 * 1024);
        // 60 requests (HEAVY threshold) ≈ 8 + 60*0.25 = 23 MB.
        assert_eq!(tab_estimate_kb(60, &cfg), 8 * 1024 + 60 * 256);
        assert_eq!(tab_estimate_kb(600, &cfg), 8 * 1024 + 600 * 256);
    }

    #[test]
    fn availability_shrinks_budget() {
        let cfg = BudgetConfig::default();
        // Plenty available → nominal budget.
        assert_eq!(total_budget_kb(Some(4 * 1024 * 1024), &cfg), 384 * 1024);
        // 200 MB available → 96 + 200*0.25 = 146 MB.
        assert_eq!(total_budget_kb(Some(200 * 1024), &cfg), 96 * 1024 + 50 * 1024);
        // Unknown availability → nominal.
        assert_eq!(total_budget_kb(None, &cfg), 384 * 1024);
        // Adaption disabled → nominal.
        let off = BudgetConfig { availability_fraction: 0.0, ..cfg };
        assert_eq!(total_budget_kb(Some(0), &off), 384 * 1024);
    }

    #[test]
    fn active_tab_gets_multiplier() {
        let cfg = BudgetConfig { active_share: 3.0, ..Default::default() };
        // 3 tabs of equal weight 32MB each, total 96MB budget.
        let weights = [32 * 1024u64; 3];
        let total = 96 * 1024;
        let active = tab_budget_kb(total, true, &weights, 0, &cfg);
        let bg = tab_budget_kb(total, false, &weights, 1, &cfg);
        // Active: pool after others (96-64=32) / 3 → floored to its
        // weight (32MB) — the floor protects it from being squeezed.
        assert_eq!(active, 32 * 1024);
        // Background: 96 * 32/96 = 32MB.
        assert_eq!(bg, 32 * 1024);
        // Heavier tab gets a bigger slice.
        let mixed = [16 * 1024u64, 64 * 1024, 16 * 1024];
        assert!(
            tab_budget_kb(total, false, &mixed, 1, &cfg)
                > tab_budget_kb(total, false, &mixed, 0, &cfg)
        );
    }

    #[test]
    fn degradation_ladder() {
        let cfg = BudgetConfig { trim_grace: Duration::from_secs(30), ..Default::default() };
        // Within budget → nothing.
        assert_eq!(decide(10 * 1024, 16 * 1024, None, 1000, &cfg), Degradation::None);
        // Over budget, never trimmed → trim.
        assert_eq!(decide(20 * 1024, 16 * 1024, None, 1000, &cfg), Degradation::TrimCaches);
        // Over budget, trimmed recently → grace period, no escalation.
        assert_eq!(decide(20 * 1024, 16 * 1024, Some(1000), 20_000, &cfg), Degradation::None);
        // Grace elapsed → hibernate.
        assert_eq!(decide(20 * 1024, 16 * 1024, Some(1000), 35_000, &cfg), Degradation::Hibernate);
    }

    #[test]
    fn js_heap_tiers() {
        assert_eq!(JsHeapTier::Normal.mem_max_mb(), 256);
        assert_eq!(JsHeapTier::Minimal.mem_max_mb(), 96);
        // Nominal everything → Normal.
        assert_eq!(JsHeapTier::for_conditions(0.5, false, 0), JsHeapTier::Normal);
        // RSS at 90% → Tight.
        assert_eq!(JsHeapTier::for_conditions(0.9, false, 0), JsHeapTier::Tight);
        // One tab over its own budget → Tight even at low RSS.
        assert_eq!(JsHeapTier::for_conditions(0.5, false, 1), JsHeapTier::Tight);
        // RSS over budget or PSI hot → Small.
        assert_eq!(JsHeapTier::for_conditions(1.1, false, 0), JsHeapTier::Small);
        assert_eq!(JsHeapTier::for_conditions(0.5, true, 0), JsHeapTier::Small);
        // PSI hot AND RSS high → Minimal.
        assert_eq!(JsHeapTier::for_conditions(0.9, true, 0), JsHeapTier::Minimal);
    }
}
