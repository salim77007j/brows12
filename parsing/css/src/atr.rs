//! At-rule model and evaluation for Brows12 CSS v0.2.
//!
//! Owns the evaluation side of `@media`, `@supports`, `@layer` and
//! `@container` (inline-size), plus owned storage for `@keyframes` and
//! `@font-face` rules. lightningcss parses; this module decides what the
//! engine's device/container environment matches.

use lightningcss::media_query::{
    MediaCondition, MediaFeatureComparison, MediaFeatureValue, MediaList, MediaType, Operator,
    Qualifier, QueryFeature,
};
use lightningcss::properties::Property;
use lightningcss::rules::container::ContainerCondition;
use lightningcss::rules::supports::SupportsCondition;
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ToCss;
use lightningcss::values::length::LengthValue;
use lightningcss::values::string::CowArcStr;
use std::collections::HashMap;

/// The device environment media queries evaluate against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceEnv {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub device_pixel_ratio: f32,
    pub dark_preferred: bool,
    pub reduced_motion: bool,
    pub pointer_fine: bool,
    pub can_hover: bool,
}

impl Default for DeviceEnv {
    fn default() -> Self {
        DeviceEnv {
            viewport_width: 1280.0,
            viewport_height: 720.0,
            device_pixel_ratio: 1.0,
            dark_preferred: false,
            reduced_motion: false,
            pointer_fine: true,
            can_hover: true,
        }
    }
}

/// A resolved feature value for comparison.
enum FeatureVal {
    Px(f32),
    Num(f32),
    Ident(String),
    Ratio(f32),
}

impl DeviceEnv {
    /// Canonical feature lookup by name (serialised from lightningcss ids).
    fn feature(&self, name: &str) -> Option<FeatureVal> {
        let name = name.to_ascii_lowercase();
        Some(match name.as_str() {
            "width" | "inline-size" => FeatureVal::Px(self.viewport_width),
            "height" | "block-size" => FeatureVal::Px(self.viewport_height),
            "aspect-ratio" => {
                FeatureVal::Ratio(self.viewport_width / self.viewport_height.max(1.0))
            }
            "orientation" => FeatureVal::Ident(
                if self.viewport_height >= self.viewport_width { "portrait" } else { "landscape" }
                    .into(),
            ),
            "resolution" => FeatureVal::Num(self.device_pixel_ratio * 96.0),
            "prefers-color-scheme" => {
                FeatureVal::Ident(if self.dark_preferred { "dark" } else { "light" }.into())
            }
            "prefers-reduced-motion" => FeatureVal::Ident(
                if self.reduced_motion { "reduce" } else { "no-preference" }.into(),
            ),
            "prefers-reduced-transparency" => FeatureVal::Ident("no-preference".into()),
            "prefers-contrast" => FeatureVal::Ident("no-preference".into()),
            "prefers-color-scheme-dark" => FeatureVal::Ident("light".into()),
            "pointer" | "any-pointer" => {
                FeatureVal::Ident(if self.pointer_fine { "fine" } else { "none" }.into())
            }
            "hover" | "any-hover" => {
                FeatureVal::Ident(if self.can_hover { "hover" } else { "none" }.into())
            }
            "display-mode" => FeatureVal::Ident("browser".into()),
            "color" => FeatureVal::Num(8.0),
            "color-gamut" => FeatureVal::Ident("srgb".into()),
            "color-index" => FeatureVal::Num(0.0),
            "monochrome" => FeatureVal::Num(0.0),
            "grid" => FeatureVal::Num(0.0),
            "scripting" => FeatureVal::Ident("enabled".into()),
            "overflow-block" | "overflow-inline" => FeatureVal::Ident("scroll".into()),
            "update" => FeatureVal::Ident("fast".into()),
            "scan" => FeatureVal::Ident("progressive".into()),
            "dynamic-range" => FeatureVal::Ident("standard".into()),
            "environment-blending" => FeatureVal::Ident("opaque".into()),
            "nav-controls" => FeatureVal::Ident("back".into()),
            "video-color-gamut" => FeatureVal::Ident("srgb".into()),
            "video-dynamic-range" => FeatureVal::Ident("standard".into()),
            "forced-colors" => FeatureVal::Ident("none".into()),
            "inverted-colors" => FeatureVal::Ident("none".into()),
            _ => return None,
        })
    }
}

fn value_to_number(v: &MediaFeatureValue) -> Option<f32> {
    Some(match v {
        MediaFeatureValue::Length(l) => match l {
            lightningcss::values::length::Length::Value(LengthValue::Px(px)) => *px,
            lightningcss::values::length::Length::Value(LengthValue::Em(e)) => *e * 16.0,
            lightningcss::values::length::Length::Value(LengthValue::Rem(r)) => *r * 16.0,
            lightningcss::values::length::Length::Value(other) => other.to_px()?,
            _ => return None,
        },
        MediaFeatureValue::Number(n) => *n,
        MediaFeatureValue::Integer(n) => *n as f32,
        MediaFeatureValue::Boolean(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        MediaFeatureValue::Resolution(r) => match r {
            lightningcss::values::resolution::Resolution::Dpi(d) => *d,
            lightningcss::values::resolution::Resolution::Dpcm(d) => d * 2.54,
            lightningcss::values::resolution::Resolution::Dppx(x) => x * 96.0,
        },
        MediaFeatureValue::Ratio(r) => r.0 / r.1.max(1.0),
        _ => return None,
    })
}

fn value_to_ident(v: &MediaFeatureValue) -> Option<String> {
    match v {
        MediaFeatureValue::Ident(i) => Some(i.to_string().to_ascii_lowercase()),
        _ => None,
    }
}

fn cmp(actual: f32, op: MediaFeatureComparison, wanted: f32) -> bool {
    use MediaFeatureComparison::*;
    match op {
        Equal => (actual - wanted).abs() < 0.001,
        GreaterThan => actual > wanted,
        GreaterThanEqual => actual >= wanted - 0.001,
        LessThan => actual < wanted,
        LessThanEqual => actual <= wanted + 0.001,
    }
}

/// Evaluate one `QueryFeature` (shared shape for @media and @container
/// size features) against a feature provider.
fn eval_feature<F, S>(feature: &QueryFeature<S>, env: &F) -> bool
where
    F: Fn(&str) -> Option<FeatureVal>,
    S: ToCss,
{
    let name = |n: &lightningcss::media_query::MediaFeatureName<S>| -> Option<String> {
        match n {
            lightningcss::media_query::MediaFeatureName::Standard(id) => {
                let mut out = String::new();
                let mut printer = lightningcss::printer::Printer::new(&mut out, Default::default());
                id.to_css(&mut printer).ok()?;
                Some(out.trim().to_ascii_lowercase())
            }
            _ => None,
        }
    };
    match feature {
        QueryFeature::Plain { name: n, value } => {
            let Some(name) = name(n) else { return false };
            let Some(actual) = env(&name) else { return false };
            match (actual, value) {
                (FeatureVal::Ident(a), v) => value_to_ident(v).map(|b| a == b).unwrap_or(false),
                (FeatureVal::Px(a), v) => {
                    value_to_number(v).map(|b| (a - b).abs() < 0.01).unwrap_or(false)
                }
                (FeatureVal::Num(a), v) => {
                    value_to_number(v).map(|b| (a - b).abs() < 0.01).unwrap_or(false)
                }
                (FeatureVal::Ratio(a), v) => {
                    value_to_number(v).map(|b| (a - b).abs() < 0.01).unwrap_or(false)
                }
            }
        }
        QueryFeature::Boolean { name: n } => {
            let Some(name) = name(n) else { return false };
            // Boolean features are true when the value != 0 / feature exists.
            match env(&name) {
                Some(FeatureVal::Num(x)) => x != 0.0,
                Some(FeatureVal::Px(x)) => x != 0.0,
                Some(FeatureVal::Ident(_)) | Some(FeatureVal::Ratio(_)) => true,
                None => false,
            }
        }
        QueryFeature::Range { name: n, operator, value } => {
            let Some(name) = name(n) else { return false };
            let actual = match env(&name) {
                Some(FeatureVal::Px(x)) | Some(FeatureVal::Num(x)) | Some(FeatureVal::Ratio(x)) => {
                    x
                }
                _ => return false,
            };
            let Some(wanted) = value_to_number(value) else { return false };
            cmp(actual, *operator, wanted)
        }
        QueryFeature::Interval { name: n, start, start_operator, end, end_operator } => {
            let Some(name) = name(n) else { return false };
            let actual = match env(&name) {
                Some(FeatureVal::Px(x)) | Some(FeatureVal::Num(x)) | Some(FeatureVal::Ratio(x)) => {
                    x
                }
                _ => return false,
            };
            let (Some(s), Some(e)) = (value_to_number(start), value_to_number(end)) else {
                return false;
            };
            // "(120px < width < 240px)": start_operator compares start↔width,
            // end_operator compares width↔end.
            use MediaFeatureComparison as C;
            let lo = match start_operator {
                C::LessThan => actual > s,
                C::LessThanEqual => actual >= s,
                C::GreaterThan => actual < s,
                C::GreaterThanEqual | C::Equal => actual <= s,
            };
            let hi = match end_operator {
                C::LessThan => actual < e,
                C::LessThanEqual => actual <= e,
                C::GreaterThan => actual > e,
                C::GreaterThanEqual | C::Equal => actual >= e,
            };
            lo && hi
        }
    }
}

/// Does a full media query list match the device?
pub fn media_list_matches(list: &MediaList, env: &DeviceEnv) -> bool {
    if list.media_queries.is_empty() {
        return true;
    }
    list.media_queries.iter().any(|q| media_query_matches(q, env))
}

fn media_query_matches(q: &lightningcss::media_query::MediaQuery, env: &DeviceEnv) -> bool {
    let type_matches = match &q.media_type {
        MediaType::All => true,
        MediaType::Screen => true, // we are a screen medium
        MediaType::Print => false,
        MediaType::Custom(_) => false,
    };
    let type_matches = match q.qualifier {
        Some(Qualifier::Not) => !type_matches,
        Some(Qualifier::Only) => type_matches,
        None => type_matches,
    };
    if !type_matches {
        return false;
    }
    match &q.condition {
        None => true,
        Some(cond) => media_condition_matches(cond, env),
    }
}

fn media_condition_matches(cond: &MediaCondition, env: &DeviceEnv) -> bool {
    match cond {
        MediaCondition::Feature(f) => eval_feature(f, &|n: &str| env.feature(n)),
        MediaCondition::Not(c) => !media_condition_matches(c, env),
        MediaCondition::Operation { operator, conditions } => match operator {
            Operator::And => conditions.iter().all(|c| media_condition_matches(c, env)),
            Operator::Or => conditions.iter().any(|c| media_condition_matches(c, env)),
        },
        MediaCondition::Unknown(_) => false,
    }
}

/// Does a `@supports` condition hold, given a property-value predicate?
pub fn supports_matches(cond: &SupportsCondition, caps: &SupportCaps) -> bool {
    match cond {
        SupportsCondition::Not(c) => !supports_matches(c, caps),
        SupportsCondition::And(v) => v.iter().all(|c| supports_matches(c, caps)),
        SupportsCondition::Or(v) => v.iter().any(|c| supports_matches(c, caps)),
        SupportsCondition::Declaration { property_id, value } => {
            caps.supports_declaration(property_id, value)
        }
        SupportsCondition::Selector(_) => true, // :has() etc. accepted optimistically
        SupportsCondition::Unknown(_) => false,
    }
}

/// Capability predicate for `@supports`.
pub struct SupportCaps;

impl SupportCaps {
    fn supports_declaration(
        &self,
        id: &lightningcss::properties::PropertyId,
        value: &CowArcStr,
    ) -> bool {
        use lightningcss::properties::PropertyId as P;
        // Known-and-applied properties only (the v0.2 applier set). Shorthand
        // ids are unit variants (the parser expands them to longhands).
        let known = matches!(
            id,
            P::Color
                | P::BackgroundColor
                | P::Display
                | P::FontSize
                | P::FontWeight
                | P::FontFamily
                | P::FontStyle
                | P::LineHeight
                | P::Margin
                | P::MarginTop
                | P::MarginRight
                | P::MarginBottom
                | P::MarginLeft
                | P::Padding
                | P::PaddingTop
                | P::PaddingRight
                | P::PaddingBottom
                | P::PaddingLeft
                | P::Width
                | P::Height
                | P::MinWidth
                | P::MaxWidth
                | P::MinHeight
                | P::MaxHeight
                | P::BorderWidth
                | P::BorderTopWidth
                | P::BorderRightWidth
                | P::BorderBottomWidth
                | P::BorderLeftWidth
                | P::BorderColor
                | P::BorderStyle
                | P::BorderRadius(_)
                | P::Opacity
                | P::TextAlign
                | P::WhiteSpace
                | P::TextDecoration(_)
                | P::TextDecorationLine(_)
                | P::FlexDirection(_)
                | P::FlexWrap(_)
                | P::Flex(_)
                | P::FlexGrow(_)
                | P::FlexShrink(_)
                | P::JustifyContent(_)
                | P::AlignItems(_)
                | P::Gap
                | P::RowGap
                | P::ColumnGap
                | P::Overflow
                | P::OverflowX
                | P::OverflowY
                | P::Position
                | P::ZIndex
                | P::Top
                | P::Right
                | P::Bottom
                | P::Left
                | P::Transform(_)
                | P::Translate
                | P::Rotate
                | P::Scale
                | P::Animation(_)
                | P::AnimationName(_)
                | P::AnimationDuration(_)
                | P::AnimationDelay(_)
                | P::AnimationIterationCount(_)
                | P::AnimationDirection(_)
                | P::AnimationFillMode(_)
                | P::AnimationTimingFunction(_)
                | P::Transition(_)
                | P::TransitionProperty(_)
                | P::TransitionDuration(_)
                | P::TransitionDelay(_)
                | P::TransitionTimingFunction(_)
                | P::ContainerType
                | P::Custom(_)
        );
        if !known {
            return false;
        }
        // The value must parse cleanly for the declaration to be supported.
        // lightningcss error-recovery stores invalid values as Custom/Unparsed
        // properties, so reject those too.
        let name = to_css_string(id).unwrap_or_default();
        let src = format!("{name}: {value}");
        lightningcss::stylesheet::StyleAttribute::parse(&src, ParserOptions::default())
            .map(|a| {
                use lightningcss::properties::Property as P;
                let clean = |p: &P| !matches!(p, P::Custom(_) | P::Unparsed(_));
                let decls = &a.declarations.declarations;
                let imp = &a.declarations.important_declarations;
                (!decls.is_empty() && decls.iter().all(clean))
                    || (!imp.is_empty() && imp.iter().all(clean))
            })
            .unwrap_or(false)
    }
}

/// Evaluate a `@container` condition against the nearest container's size.
/// `container_inline_size` is the resolved inline size of the nearest
/// matching container, when known.
pub fn container_condition_matches(
    cond: &ContainerCondition,
    container_inline_size: Option<f32>,
    container_block_size: Option<f32>,
) -> bool {
    match cond {
        ContainerCondition::Not(c) => {
            !container_condition_matches(c, container_inline_size, container_block_size)
        }
        ContainerCondition::Operation { operator, conditions } => match operator {
            lightningcss::media_query::Operator::And => conditions.iter().all(|c| {
                container_condition_matches(c, container_inline_size, container_block_size)
            }),
            lightningcss::media_query::Operator::Or => conditions.iter().any(|c| {
                container_condition_matches(c, container_inline_size, container_block_size)
            }),
        },
        ContainerCondition::Style(_) => false, // style queries: not supported
        ContainerCondition::ScrollState(_) => false,
        ContainerCondition::Unknown(_) => false,
        ContainerCondition::Feature(f) => {
            let (Some(w), Some(_h)) = (container_inline_size, container_block_size) else {
                return false;
            };
            eval_feature(f, &|name: &str| {
                let name = name.to_ascii_lowercase();
                Some(match name.as_str() {
                    "width" | "inline-size" => FeatureVal::Px(w),
                    _ => return None,
                })
            })
        }
    }
}

/// An owned `@keyframes` rule.
#[derive(Debug, Clone)]
pub struct OwnedKeyframes {
    pub name: String,
    pub frames: Vec<OwnedKeyframe>,
}

#[derive(Debug, Clone)]
pub struct OwnedKeyframe {
    /// Normalised offset in `0.0..=1.0`.
    pub offset: f32,
    pub declarations: Vec<Property<'static>>,
    pub important: Vec<Property<'static>>,
    /// `animation-timing-function` declared inside the keyframe.
    pub easing: Option<EasingKeyword>,
}

/// Easing keywords we interpolate with (cubic-bezier backed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EasingKeyword {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    StepStart,
    StepEnd,
}

impl EasingKeyword {
    pub fn eval(&self, t: f32) -> f32 {
        match self {
            EasingKeyword::Linear => t,
            EasingKeyword::Ease => cubic_bezier(0.25, 0.1, 0.25, 1.0, t),
            EasingKeyword::EaseIn => cubic_bezier(0.42, 0.0, 1.0, 1.0, t),
            EasingKeyword::EaseOut => cubic_bezier(0.0, 0.0, 0.58, 1.0, t),
            EasingKeyword::EaseInOut => cubic_bezier(0.42, 0.0, 0.58, 1.0, t),
            EasingKeyword::StepStart => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            EasingKeyword::StepEnd => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

/// Solve a CSS cubic-bezier timing function for progress `t` in 0..=1.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bez_x =
        |u: f32| 3.0 * u * (1.0 - u) * (1.0 - u) * x1 + 3.0 * u * u * (1.0 - u) * x2 + u * u * u;
    let bez_y =
        |u: f32| 3.0 * u * (1.0 - u) * (1.0 - u) * y1 + 3.0 * u * u * (1.0 - u) * y2 + u * u * u;
    // Newton-Raphson on x(u) = t, fall back to bisection.
    let mut u = t;
    for _ in 0..8 {
        let x = bez_x(u) - t;
        if x.abs() < 1e-5 {
            break;
        }
        let d = 3.0 * (1.0 - u) * (1.0 - u) * x1
            + 6.0 * u * (1.0 - u) * (x2 - x1)
            + 3.0 * u * u * (1.0 - x2);
        if d.abs() < 1e-6 {
            break;
        }
        u -= x / d;
        u = u.clamp(0.0, 1.0);
    }
    if (bez_x(u) - t).abs() > 1e-4 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = (lo + hi) / 2.0;
            if bez_x(mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        u = (lo + hi) / 2.0;
    }
    bez_y(u)
}

/// An owned `@font-face` rule (engine loads the first usable source).
#[derive(Debug, Clone)]
pub struct OwnedFontFace {
    pub family: String,
    pub urls: Vec<String>,
    pub weight: Option<u16>,
    pub style_italic: bool,
}

/// Layer order registry: records first-appearance order of layer paths.
#[derive(Debug, Clone, Default)]
pub struct LayerRegistry {
    pub order: Vec<String>,
}

impl LayerRegistry {
    pub fn declare(&mut self, path: &str) {
        if !self.order.iter().any(|p| p == path) {
            self.order.push(path.to_string());
        }
    }

    /// Rank for normal declarations: unlayered wins (MAX), then declared order.
    pub fn rank_normal(&self, layer: &Option<String>) -> u32 {
        match layer {
            None => u32::MAX,
            Some(p) => {
                self.order.iter().position(|o| o == p).map(|i| i as u32).unwrap_or(u32::MAX - 1)
            }
        }
    }

    /// Rank for important declarations: reversed (earlier layer wins,
    /// unlayered loses to layered).
    pub fn rank_important(&self, layer: &Option<String>) -> u32 {
        match layer {
            Some(p) => self
                .order
                .iter()
                .position(|o| o == p)
                .map(|i| u32::MAX - i as u32)
                .unwrap_or(u32::MAX),
            None => u32::MAX,
        }
    }
}

/// Map a lightningcss timing function onto our easing keywords.
pub(crate) fn timing_to_easing(
    tf: &lightningcss::values::easing::EasingFunction,
) -> Option<EasingKeyword> {
    use lightningcss::values::easing::EasingFunction as TF;
    match tf {
        TF::Linear => Some(EasingKeyword::Linear),
        TF::Ease => Some(EasingKeyword::Ease),
        TF::EaseIn => Some(EasingKeyword::EaseIn),
        TF::EaseOut => Some(EasingKeyword::EaseOut),
        TF::EaseInOut => Some(EasingKeyword::EaseInOut),
        TF::CubicBezier { x1, y1, x2, y2 } => {
            let (x1, y1, x2, y2) = (*x1, *y1, *x2, *y2);
            if (x1, y1, x2, y2) == (0.25, 0.1, 0.25, 1.0) {
                Some(EasingKeyword::Ease)
            } else if (x1, y1, x2, y2) == (0.42, 0.0, 1.0, 1.0) {
                Some(EasingKeyword::EaseIn)
            } else if (x1, y1, x2, y2) == (0.0, 0.0, 0.58, 1.0) {
                Some(EasingKeyword::EaseOut)
            } else if (x1, y1, x2, y2) == (0.42, 0.0, 0.58, 1.0) {
                Some(EasingKeyword::EaseInOut)
            } else if (x1, y1) == (0.0, 0.0) && (x2, y2) == (1.0, 1.0) {
                Some(EasingKeyword::Linear)
            } else {
                // Arbitrary bezier: approximate with the closest canonical.
                Some(EasingKeyword::Ease)
            }
        }
        TF::Steps { .. } => Some(EasingKeyword::StepEnd),
    }
}

/// Serialize helper for any ToCss value into a compact string.
pub(crate) fn to_css_string<T: ToCss>(v: &T) -> Option<String> {
    let mut out = String::new();
    let mut printer = lightningcss::printer::Printer::new(
        &mut out,
        lightningcss::printer::PrinterOptions { minify: true, ..Default::default() },
    );
    v.to_css(&mut printer).ok()?;
    Some(out)
}

/// Keyframes lookup shared by the animation engine.
pub type KeyframesMap = HashMap<String, OwnedKeyframes>;
