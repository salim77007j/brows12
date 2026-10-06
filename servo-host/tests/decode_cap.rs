//! v2.1 Phase 1.1 / 2.1 — tiered image decode: tests for the public
//! surface of the `servo-net` fork's display-bound decode cap.
//!
//! The fork (`patched/servo-net`, `image_cache.rs`) rescales any decoded
//! raster exceeding the physical display bound (viewport x DPR; env
//! `BROWS12_IMAGE_DECODE_MAX_W/H`, default 2560x1600, 0 disables) before
//! the image enters the image cache / WebRender texture cache. The
//! decision logic is `DecodeCap`; the rescale itself is verified
//! end-to-end by the A/B peak-RSS harness (scripts/p11_cnn_diag.py) and
//! the side-by-side screenshot battery.

// The published crate `servo-net` exposes its library under the
// name `net` (see patched/servo-net/Cargo.toml [lib] name).
use net::image_cache::DecodeCap;

#[test]
fn default_cap_is_viewport_x_dpr2() {
    let cap = DecodeCap { max_w: 2560, max_h: 1600 };
    assert!(cap.active());
    // At or under the cap: untouched.
    assert!(cap.factor_for(1280, 800).is_none());
    assert!(cap.factor_for(2560, 1600).is_none());
    // Over either bound: scaled by the binding constraint.
    assert!(cap.factor_for(4000, 2000).is_some());
    assert!(cap.factor_for(1000, 3200).is_some());
}

#[test]
fn zero_disables_the_cap() {
    let cap = DecodeCap { max_w: 0, max_h: 1600 };
    assert!(!cap.active());
    assert!(cap.factor_for(8000, 4000).is_none());
}

#[test]
fn factor_is_the_min_constraint_and_uniform() {
    let cap = DecodeCap { max_w: 2560, max_h: 1600 };
    // 5120x1000 -> w-bound: 0.5 -> 2560x500.
    let f = cap.factor_for(5120, 1000).expect("over cap");
    assert!((f - 0.5).abs() < 1e-9);
    // h-bound case: 1000x4000 -> 0.4.
    let f = cap.factor_for(1000, 4000).expect("over cap");
    assert!((f - 0.4).abs() < 1e-9);
    // Degenerate input never scales.
    assert!(cap.factor_for(0, 0).is_none());
}

#[test]
fn decoded_bytes_shrink_by_the_expected_ratio() {
    // The whole point: a 4000x2000 RGBA8 hero costs 32 MB decoded; after
    // the cap it must cost 2560x1280x4 = 13.1 MB.
    let cap = DecodeCap { max_w: 2560, max_h: 1600 };
    let f = cap.factor_for(4000, 2000).unwrap();
    let new_w = (4000.0_f64 * f).round() as u32;
    let new_h = (2000.0_f64 * f).round() as u32;
    assert_eq!((new_w, new_h), (2560, 1280));
    assert_eq!(new_w as u64 * new_h as u64 * 4, 13_107_200);
}
