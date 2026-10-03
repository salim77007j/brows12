//! Animation: `@keyframes` playback and CSS transitions.
//!
//! The engine drives a clock; each tick calls [`apply_keyframes`] for every
//! animated element and the [`TransitionEngine`] for elements whose style
//! changed. Interpolation is property-typed (colors, lengths, opacity,
//! transforms), with step fallback for non-interpolable pairs.

use crate::apply::apply_property;
use crate::atr::{EasingKeyword, KeyframesMap, OwnedKeyframes};
use crate::computed::{CascadeCtx, ComputedStyle};
use crate::values::{AnimationFill, AnimationSpec, Len, Transform};
use brows12_html::{Document, NodeId};
use std::collections::HashMap;

/// Per-element playback state the engine owns.
#[derive(Debug, Clone)]
pub struct AnimationState {
    pub started_s: f32,
}

/// Result of applying animations at a point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationStatus {
    /// At least one animation still running (engine should keep ticking).
    Active,
    /// Nothing running (engine can stop ticking).
    Idle,
}

/// Apply keyframe animations for one element onto `style`.
///
/// `base` is the cascade result (the underlying value); keyframes composite
/// over it. Returns [`AnimationStatus::Active`] while the animation runs.
pub fn apply_keyframes(
    style: &mut ComputedStyle,
    base: &ComputedStyle,
    kf: &OwnedKeyframes,
    spec: &AnimationSpec,
    elapsed_s: f32,
    ctx: &CascadeCtx,
    parent: Option<&ComputedStyle>,
) -> AnimationStatus {
    let duration = spec.duration_s.max(0.0);
    let delay = spec.delay_s.max(0.0);
    let local = elapsed_s - delay;

    #[allow(clippy::float_cmp)]
    if duration <= 0.0 {
        // Zero-duration: jump to the final frame (respecting fill).
        if matches!(spec.fill, AnimationFill::Forwards | AnimationFill::Both) && local >= 0.0 {
            apply_frame_at(style, base, kf, 1.0, ctx, parent);
            return AnimationStatus::Idle;
        }
        return AnimationStatus::Idle;
    }

    if local < 0.0 {
        // In the delay phase.
        if matches!(spec.fill, AnimationFill::Backwards | AnimationFill::Both) {
            apply_frame_at(style, base, kf, 0.0, ctx, parent);
        }
        return AnimationStatus::Active;
    }

    let infinite = spec.iteration.is_infinite();
    let iterations = match spec.iteration {
        crate::values::IterationCount::Number(n) => n.max(0.0),
        crate::values::IterationCount::One => 1.0,
        crate::values::IterationCount::Infinite => f32::INFINITY,
    };

    let raw = local / duration;
    let iteration = raw.floor();
    let mut pos = raw - iteration;

    if !infinite && iteration >= iterations {
        // Finished.
        if matches!(spec.fill, AnimationFill::Forwards | AnimationFill::Both) {
            apply_frame_at(style, base, kf, 1.0, ctx, parent);
            return AnimationStatus::Idle;
        }
        return AnimationStatus::Idle;
    }

    // Direction handling.
    let flip = match spec.direction {
        crate::values::AnimationDirection::Normal => false,
        crate::values::AnimationDirection::Reverse => true,
        crate::values::AnimationDirection::Alternate => (iteration as i64) % 2 == 1,
        crate::values::AnimationDirection::AlternateReverse => (iteration as i64) % 2 == 0,
    };
    if flip {
        pos = 1.0 - pos;
    }

    // Per-keyframe easing: the start frame's easing governs its segment.
    let eased = eased_progress(kf, pos);
    apply_progress(style, base, kf, eased, ctx, parent);
    AnimationStatus::Active
}

fn eased_progress(kf: &OwnedKeyframes, pos: f32) -> f32 {
    // Find the segment containing pos; apply the START frame's easing.
    let mut start_easing: Option<EasingKeyword> = None;
    let mut seg_start = 0.0f32;
    let mut seg_end = 1.0f32;
    let mut found = false;
    for (i, f) in kf.frames.iter().enumerate() {
        if f.offset >= pos {
            seg_end = f.offset;
            if i > 0 {
                seg_start = kf.frames[i - 1].offset;
                start_easing = kf.frames[i - 1].easing;
            }
            found = true;
            break;
        }
    }
    if !found {
        // Past the last keyframe: hold position.
        return pos;
    }
    let span = (seg_end - seg_start).max(f32::EPSILON);
    let local = ((pos - seg_start) / span).clamp(0.0, 1.0);
    match start_easing {
        Some(e) => seg_start + e.eval(local) * span,
        None => pos,
    }
}

/// Apply the interpolated property set at progress `pos` onto `style`.
fn apply_progress(
    style: &mut ComputedStyle,
    base: &ComputedStyle,
    kf: &OwnedKeyframes,
    pos: f32,
    ctx: &CascadeCtx,
    parent: Option<&ComputedStyle>,
) {
    // Gather every property name present in the keyframes.
    let mut names: Vec<String> = Vec::new();
    for f in &kf.frames {
        for p in f.declarations.iter().chain(f.important.iter()) {
            let n = crate::atr::to_css_string(&p.property_id()).unwrap_or_default();
            if !n.is_empty() && !names.contains(&n) {
                names.push(n);
            }
        }
    }

    for name in names {
        // Build the declaration sequence for this property.
        let mut decls: Vec<(f32, &lightningcss::properties::Property)> = Vec::new();
        for f in &kf.frames {
            let found = f.declarations.iter().chain(f.important.iter()).find(|p| {
                crate::atr::to_css_string(&p.property_id()).as_deref() == Some(name.as_str())
            });
            if let Some(p) = found {
                decls.push((f.offset, p));
            }
        }
        if decls.is_empty() {
            continue;
        }
        decls.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        if pos < decls[0].0 {
            // Before the first frame that declares this property: underlying
            // value wins (no application).
            continue;
        }
        // Find bracketing pair.
        let mut applied = false;
        for w in decls.windows(2) {
            let (a_off, a) = (w[0].0, w[0].1);
            let (b_off, b) = (w[1].0, w[1].1);
            if pos >= a_off && pos <= b_off {
                let t = if (b_off - a_off).abs() < f32::EPSILON {
                    1.0
                } else {
                    ((pos - a_off) / (b_off - a_off)).clamp(0.0, 1.0)
                };
                match lerp_pair(a, b, t) {
                    Some(p) => {
                        apply_property(style, &p, parent, ctx);
                    }
                    None => {
                        // Non-interpolable: step.
                        let chosen = if t >= 0.5 { b } else { a };
                        apply_property(style, chosen, parent, ctx);
                    }
                }
                applied = true;
                break;
            }
        }
        if !applied {
            // Past the last declaring frame: hold the last value.
            let (_, last) = decls.last().unwrap();
            apply_property(style, last, parent, ctx);
        }
        let _ = base;
    }
}

/// Apply the state at an exact endpoint (0.0 or 1.0).
fn apply_frame_at(
    style: &mut ComputedStyle,
    base: &ComputedStyle,
    kf: &OwnedKeyframes,
    at: f32,
    ctx: &CascadeCtx,
    parent: Option<&ComputedStyle>,
) {
    apply_progress(style, base, kf, at, ctx, parent);
}

/// Interpolate two declarations of the same property. `None` = not
/// interpolable by this engine (caller steps).
pub fn lerp_pair(
    a: &lightningcss::properties::Property<'static>,
    b: &lightningcss::properties::Property<'static>,
    t: f32,
) -> Option<lightningcss::properties::Property<'static>> {
    use lightningcss::properties::Property as P;
    let lerp_f32 = |x: f32, y: f32| x + (y - x) * t;
    let _lerp_len = |x: Len, y: Len| -> Option<Len> {
        match (x, y) {
            (Len::Px(a), Len::Px(b)) => Some(Len::Px(lerp_f32(a, b))),
            _ => None,
        }
    };
    let lerp_lp = |x: &lightningcss::values::length::LengthPercentage,
                   y: &lightningcss::values::length::LengthPercentage|
     -> Option<lightningcss::values::length::LengthPercentage> {
        use lightningcss::values::length::LengthValue as LV;
        use lightningcss::values::percentage::DimensionPercentage as DP;
        match (x, y) {
            (DP::Dimension(LV::Px(a)), DP::Dimension(LV::Px(b))) => {
                Some(DP::Dimension(LV::Px(lerp_f32(*a, *b))))
            }
            _ => None,
        }
    };
    let lerp_rgba = |x: crate::values::Rgba, y: crate::values::Rgba| -> crate::values::Rgba {
        [
            lerp_f32(x[0] as f32, y[0] as f32).round() as u8,
            lerp_f32(x[1] as f32, y[1] as f32).round() as u8,
            lerp_f32(x[2] as f32, y[2] as f32).round() as u8,
            lerp_f32(x[3] as f32, y[3] as f32).round() as u8,
        ]
    };
    let rgba_color = |c: &lightningcss::values::color::CssColor| -> Option<crate::values::Rgba> {
        crate::stylesheet::resolve_color(c, [0, 0, 0, 255])
    };
    let mk_rgba = |r: crate::values::Rgba| -> lightningcss::values::color::CssColor {
        use lightningcss::values::color::{CssColor, RGBA};
        CssColor::RGBA(RGBA { red: r[0], green: r[1], blue: r[2], alpha: r[3] })
    };

    match (a, b) {
        (P::Opacity(x), P::Opacity(y)) => {
            Some(P::Opacity(lightningcss::values::alpha::AlphaValue(lerp_f32(x.0, y.0))))
        }
        (P::Color(x), P::Color(y)) => {
            let (Some(xr), Some(yr)) = (rgba_color(x), rgba_color(y)) else { return None };
            Some(P::Color(mk_rgba(lerp_rgba(xr, yr))))
        }
        (P::BackgroundColor(x), P::BackgroundColor(y)) => {
            let (Some(xr), Some(yr)) = (rgba_color(x), rgba_color(y)) else { return None };
            Some(P::BackgroundColor(mk_rgba(lerp_rgba(xr, yr))))
        }
        (P::Width(x), P::Width(y)) => {
            use lightningcss::properties::size::Size;
            match (x, y) {
                (Size::LengthPercentage(a), Size::LengthPercentage(b)) => {
                    lerp_lp(a, b).map(|l| P::Width(Size::LengthPercentage(l)))
                }
                _ => None,
            }
        }
        (P::Height(x), P::Height(y)) => {
            use lightningcss::properties::size::Size;
            match (x, y) {
                (Size::LengthPercentage(a), Size::LengthPercentage(b)) => {
                    lerp_lp(a, b).map(|l| P::Height(Size::LengthPercentage(l)))
                }
                _ => None,
            }
        }
        (P::FontSize(x), P::FontSize(y)) => {
            use lightningcss::properties::font::FontSize as FS;
            match (x, y) {
                (FS::Length(a), FS::Length(b)) => {
                    use lightningcss::values::length::LengthValue as LV;
                    use lightningcss::values::percentage::DimensionPercentage as DP;
                    match (a, b) {
                        (DP::Dimension(LV::Px(pa)), DP::Dimension(LV::Px(pb))) => {
                            Some(P::FontSize(FS::Length(DP::Dimension(LV::Px(lerp_f32(*pa, *pb))))))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        (P::LineHeight(x), P::LineHeight(y)) => {
            use lightningcss::properties::font::LineHeight as LH;
            use lightningcss::values::length::LengthValue as LV;
            use lightningcss::values::percentage::DimensionPercentage as DP;
            match (x, y) {
                (LH::Length(DP::Dimension(LV::Px(pa))), LH::Length(DP::Dimension(LV::Px(pb)))) => {
                    Some(P::LineHeight(LH::Length(DP::Dimension(LV::Px(lerp_f32(*pa, *pb))))))
                }
                _ => None,
            }
        }
        (P::Transform(_x, _), P::Transform(_y, _)) => {
            // Decompose both, lerp the compound, rebuild a translate+scale+rotate matrix.
            let mut sa = ComputedStyle::default();
            let mut sb = ComputedStyle::default();
            let ctx = CascadeCtx::default();
            apply_property(&mut sa, a, None, &ctx);
            apply_property(&mut sb, b, None, &ctx);
            let ta: Transform = sa.transform;
            let tb: Transform = sb.transform;
            let m = Transform {
                tx: lerp_f32(ta.tx, tb.tx),
                ty: lerp_f32(ta.ty, tb.ty),
                scale: lerp_f32(ta.scale, tb.scale),
                rotate_deg: lerp_f32(ta.rotate_deg, tb.rotate_deg),
            };
            use lightningcss::properties::transform::{
                Matrix as LcMatrix, Transform as LcTransform, TransformList as LcTransformList,
            };
            use lightningcss::vendor_prefix::VendorPrefix;
            let rad = m.rotate_deg.to_radians();
            let (s, c) = (rad.sin(), rad.cos());
            let sc = m.scale;
            Some(P::Transform(
                LcTransformList(vec![LcTransform::Matrix(LcMatrix {
                    a: c * sc,
                    b: s * sc,
                    c: -s * sc,
                    d: c * sc,
                    e: m.tx,
                    f: m.ty,
                })]),
                VendorPrefix::None,
            ))
        }
        _ => None,
    }
}

/// Tracks in-flight CSS transitions between style passes.
#[derive(Debug, Default)]
pub struct TransitionEngine {
    active: HashMap<NodeId, Vec<ActiveTransition>>,
}

#[derive(Debug, Clone)]
struct ActiveTransition {
    property_name: String,
    from: lightningcss::properties::Property<'static>,
    to: lightningcss::properties::Property<'static>,
    start_s: f32,
    duration_s: f32,
    easing: EasingKeyword,
}

/// Snapshot one property out of a computed style (for transition `from`).
fn snapshot_property(
    style: &ComputedStyle,
    name: &str,
) -> Option<lightningcss::properties::Property<'static>> {
    use lightningcss::properties::Property as P;
    use lightningcss::values::color::{CssColor, RGBA};
    use lightningcss::values::length::LengthValue;
    let mk = |r: crate::values::Rgba| {
        CssColor::RGBA(RGBA { red: r[0], green: r[1], blue: r[2], alpha: r[3] })
    };
    match name {
        "color" => Some(P::Color(mk(style.color))),
        "background-color" => Some(P::BackgroundColor(mk(style.background_color))),
        "opacity" => Some(P::Opacity(lightningcss::values::alpha::AlphaValue(style.opacity))),
        "font-size" => Some(P::FontSize(lightningcss::properties::font::FontSize::Length(
            lightningcss::values::percentage::DimensionPercentage::Dimension(LengthValue::Px(
                style.font_size,
            )),
        ))),
        "width" if !matches!(style.width, crate::values::AutoPx::Auto) => match style.width {
            crate::values::AutoPx::Len(Len::Px(px)) => {
                Some(P::Width(crate::values::AutoPx::Len(Len::Px(px)).into_width()))
            }
            _ => None,
        },
        "transform" => {
            let m = &style.transform;
            if m.is_identity() {
                return None;
            }
            let rad = m.rotate_deg.to_radians();
            let (s, c) = (rad.sin(), rad.cos());
            use lightningcss::properties::transform::{
                Matrix as LcMatrix, Transform as LcTransform, TransformList as LcTransformList,
            };
            use lightningcss::vendor_prefix::VendorPrefix;
            Some(P::Transform(
                LcTransformList(vec![LcTransform::Matrix(LcMatrix {
                    a: c * m.scale,
                    b: s * m.scale,
                    c: -s * m.scale,
                    d: c * m.scale,
                    e: m.tx,
                    f: m.ty,
                })]),
                VendorPrefix::None,
            ))
        }
        _ => None,
    }
}

impl crate::values::AutoPx {
    fn into_width(self) -> lightningcss::properties::size::Size {
        use lightningcss::properties::size::Size;
        use lightningcss::values::length::LengthValue;
        use lightningcss::values::percentage::DimensionPercentage as DP;
        match self {
            crate::values::AutoPx::Len(Len::Px(px)) => {
                Size::LengthPercentage(DP::Dimension(LengthValue::Px(px)))
            }
            crate::values::AutoPx::Len(Len::Percent(p)) => Size::LengthPercentage(DP::Percentage(
                lightningcss::values::percentage::Percentage(p),
            )),
            crate::values::AutoPx::Len(Len::Calc(c)) => Size::LengthPercentage(DP::Dimension(
                LengthValue::Px(c.px_part()),
            )),
            crate::values::AutoPx::Auto => Size::Auto,
        }
    }
}

impl TransitionEngine {
    /// Diff `prev` vs `next` for a node and start transitions per its specs.
    pub fn observe(
        &mut self,
        node: NodeId,
        prev: &ComputedStyle,
        next: &ComputedStyle,
        now_s: f32,
    ) {
        if prev == next {
            return;
        }
        let tracked = ["color", "background-color", "opacity", "font-size", "width", "transform"];
        for spec in &next.transitions {
            if spec.duration_s <= 0.0 && spec.delay_s <= 0.0 {
                continue;
            }
            for name in tracked {
                if spec.property != "all" && spec.property != name {
                    continue;
                }
                let (Some(from), Some(to)) =
                    (snapshot_property(prev, name), snapshot_property(next, name))
                else {
                    continue;
                };
                if from == to {
                    continue;
                }
                let entry = self.active.entry(node).or_default();
                entry.retain(|t| t.property_name != name);
                entry.push(ActiveTransition {
                    property_name: name.to_string(),
                    from,
                    to,
                    start_s: now_s + spec.delay_s.max(0.0),
                    duration_s: spec.duration_s.max(0.0),
                    easing: spec.easing,
                });
            }
        }
    }

    /// Apply active transitions for `node` onto `style` at `now_s`.
    pub fn apply(
        &self,
        node: NodeId,
        style: &mut ComputedStyle,
        ctx: &CascadeCtx,
        parent: Option<&ComputedStyle>,
        now_s: f32,
    ) {
        let Some(transitions) = self.active.get(&node) else { return };
        for t in transitions {
            if now_s < t.start_s {
                apply_property(style, &t.from, parent, ctx);
                continue;
            }
            let raw = if t.duration_s <= 0.0 {
                1.0
            } else {
                ((now_s - t.start_s) / t.duration_s).clamp(0.0, 1.0)
            };
            let eased = t.easing.eval(raw);
            match lerp_pair(&t.from, &t.to, eased) {
                Some(p) => apply_property(style, &p, parent, ctx),
                None => {
                    let chosen = if eased >= 0.5 { t.to.clone() } else { t.from.clone() };
                    apply_property(style, &chosen, parent, ctx);
                }
            }
        }
    }

    /// Drop finished transitions; true if any remain active.
    pub fn gc(&mut self, now_s: f32) -> bool {
        self.active.retain(|_, v| {
            v.retain(|t| now_s < t.start_s + t.duration_s + 0.05);
            !v.is_empty()
        });
        !self.active.is_empty()
    }

    pub fn clear(&mut self) {
        self.active.clear();
    }
}

/// Are any keyframe animations currently running for this style?
pub fn style_has_active_animation(style: &ComputedStyle) -> bool {
    style.animation.is_some()
}

/// Convenience lookup: keyframes by animation name.
pub fn find_keyframes<'a>(map: &'a KeyframesMap, name: &str) -> Option<&'a OwnedKeyframes> {
    map.get(name)
}

/// Helper for tests and the engine: does `doc` node have an animation spec?
pub fn animation_of(style: &ComputedStyle) -> Option<&AnimationSpec> {
    style.animation.as_ref()
}

// Import kept for doc examples.
#[allow(unused_imports)]
use crate::values::AnimationFill as _AF;

#[allow(unused)]
fn _unused(doc: &Document) -> Document {
    doc.clone()
}
