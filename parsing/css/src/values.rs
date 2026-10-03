//! Owned value types shared between the cascade, layout and rendering.

/// A symbolic CSS math expression (`calc()`, `min()`, `max()`, `clamp()`)
/// kept unresolved so the layout engine can supply the percentage base
/// (containing-block width, font size, ...) at used-value time — exactly
/// how the CSS Values 4 spec models it.
#[derive(Debug, Clone, PartialEq)]
pub enum CalcExpr {
    Px(f32),
    Percent(f32),
    Sum(Box<CalcExpr>, Box<CalcExpr>),
    /// Coefficient product: `number * expr` (calc products always have at
    /// least one number side after parsing).
    Product(f32, Box<CalcExpr>),
    Min(Vec<CalcExpr>),
    Max(Vec<CalcExpr>),
    /// clamp(MIN, VAL, MAX)
    Clamp(Box<CalcExpr>, Box<CalcExpr>, Box<CalcExpr>),
}

impl CalcExpr {
    /// Resolve against a percentage base (the length the percent part is
    /// taken against). Mixed px/percent sums evaluate exactly.
    pub fn resolve(&self, base: f32) -> f32 {
        match self {
            CalcExpr::Px(v) => *v,
            CalcExpr::Percent(p) => *p * base,
            CalcExpr::Sum(a, b) => a.resolve(base) + b.resolve(base),
            CalcExpr::Product(n, v) => *n * v.resolve(base),
            CalcExpr::Min(args) => {
                args.iter().map(|a| a.resolve(base)).fold(f32::INFINITY, f32::min)
            }
            CalcExpr::Max(args) => {
                args.iter().map(|a| a.resolve(base)).fold(f32::NEG_INFINITY, f32::max)
            }
            CalcExpr::Clamp(min, val, max) => {
                let m = min.resolve(base);
                let v = val.resolve(base);
                let x = max.resolve(base);
                v.max(m).min(x)
            }
        }
    }

    /// Best-effort px component for consumers with no percentage base
    /// (fallback paths). Mixed expressions contribute only their px part.
    pub fn px_part(&self) -> f32 {
        match self {
            CalcExpr::Px(v) => *v,
            CalcExpr::Percent(_) => 0.0,
            CalcExpr::Sum(a, b) => a.px_part() + b.px_part(),
            CalcExpr::Product(n, v) => *n * v.px_part(),
            CalcExpr::Min(args) | CalcExpr::Max(args) => {
                args.first().map(|a| a.px_part()).unwrap_or(0.0)
            }
            CalcExpr::Clamp(_, val, _) => val.px_part(),
        }
    }
}

/// A resolved horizontal/vertical length: absolute pixels, a percentage
/// (left symbolic so the layout engine can resolve it against the used
/// available space, exactly like the CSS layout algorithms expect), or a
/// symbolic calc() expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Len {
    Px(f32),
    Percent(f32),
    Calc(CalcExpr),
}

impl Len {
    /// Resolve to px against the given percentage base.
    pub fn resolve(&self, base: f32) -> f32 {
        match self {
            Len::Px(v) => *v,
            Len::Percent(p) => *p * base,
            Len::Calc(c) => c.resolve(base),
        }
    }
}

/// A length or the `auto` keyword.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoPx {
    Auto,
    Len(Len),
}

/// Per-edge box values (top, right, bottom, left).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Clone> Edges<T> {
    /// Same value on all four edges.
    pub fn splat(v: T) -> Self {
        Edges { left: v.clone(), bottom: v.clone(), right: v.clone(), top: v }
    }
}

impl Edges<Len> {
    pub fn zero() -> Self {
        Self::splat(Len::Px(0.0))
    }
}

/// `display` values understood by the layout engine (v1: block, flex,
/// inline-mapped-to-block and none; see docs/ROADMAP.md for inline flow).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Display {
    #[default]
    Block,
    Inline,
    /// `display: inline-block`: shrink-to-fit box (max-content when width
    /// is auto). In block flow v1 stacks it (atomic inline placement is a
    /// documented gap); inside flex/grid/table rows it content-sizes like
    /// Chromium.
    InlineBlock,
    Flex,
    /// `display: inline-flex`: an inline-level flex container. It lays its
    /// children out with flexbox but sizes shrink-to-fit (max-content when
    /// width is auto) and participates in flow as an atomic box, like
    /// inline-block. Pill/tag/button groups on real sites rely on it.
    InlineFlex,
    None,
    /// CSS table boxes are laid out as anonymous flex structures
    /// (table → column flex, row → row flex, cell → flex item).
    Table,
    TableRow,
    TableCell,
    /// CSS grid: template/auto tracks and item placement map onto taffy's
    /// native grid implementation.
    Grid,
}

/// One track sizing function of a CSS grid template (`grid-template-*`,
/// `grid-auto-*`). `fr`, min/max-content and `minmax()` are kept symbolic;
/// the layout engine maps them onto taffy's native grid sizing.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum GridTrackSize {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
    Fr(f32),
    MinContent,
    MaxContent,
    /// `minmax(min, max)` — either side may itself be a keyword/length.
    MinMax {
        min: Box<GridTrackSize>,
        max: Box<GridTrackSize>,
    },
}

/// A `<grid-line>`: auto placement, the Nth line (1-based; negative counts
/// from the end), or a span across N tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridLineSpec {
    #[default]
    Auto,
    Line(i16),
    Span(u16),
}

/// `float` — takes a box out of normal flow and shifts it left/right
/// (CSS 2.1 §9.5). lightningcss does not model this property, so the
/// declaration is rewritten to `--brows-float` at parse time and resolved
/// from the custom-property map during cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FloatSide {
    #[default]
    None,
    Left,
    Right,
}

/// `clear` — forbid boxes with matching floats beside an element
/// (CSS 2.1 §9.5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClearSide {
    #[default]
    None,
    Left,
    Right,
    Both,
}

/// Box positioning scheme. v1 performs static flow layout only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Start,
    Left,
    Center,
    Right,
    Justify,
}

/// A CSS gradient paint (linear or radial, non-repeating v1).
#[derive(Debug, Clone, PartialEq)]
pub enum Gradient {
    /// CSS angle: 0deg points up, 90deg points right.
    Linear { angle_deg: f32, stops: Vec<GradientStop> },
    /// Circle, farthest-corner, centered (v1 approximation).
    Radial { stops: Vec<GradientStop> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientStop {
    pub color: Rgba,
    /// 0..1 or None (evenly distributed among unpositioned stops).
    pub position: Option<f32>,
}

/// CSS `text-transform` (v1: the case keywords).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhiteSpace {
    #[default]
    Normal,
    NoWrap,
    Pre,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowKeyword {
    #[default]
    Visible,
    Hidden,
    Scroll,
    Auto,
}

/// `visibility` (CSS 2.1 §11.2): `hidden` boxes keep their layout space
/// but do not paint; descendants may restore painting with `visible`.
/// `collapse` behaves like `hidden` outside table row/column contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
    Collapse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JustifyContent {
    #[default]
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Start,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignItems {
    #[default]
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
    Start,
    End,
}

/// `flex-basis`: the initial main size of a flex item before free space
/// is distributed. `Auto` defers to the `width`/`height` property (and,
/// when that is also auto, to the item's content size); `Content` always
/// sizes to the content. Percentages resolve against the flex container's
/// inner main size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FlexBasis {
    #[default]
    Auto,
    /// `flex-basis: content` (and the deferred part of `flex: 1 auto`)
    Content,
    Px(f32),
    Percent(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FlexBox {
    pub grow: f32,
    pub shrink: f32,
    pub basis: FlexBasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ZIndex {
    #[default]
    Auto,
    Number(i32),
}

/// Line height: a unitless multiplier of font size or an absolute px value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f32),
    Px(f32),
}

/// RGBA color, 8 bit per channel.
pub type Rgba = [u8; 4];

pub const TRANSPARENT: Rgba = [0, 0, 0, 0];

use crate::atr::EasingKeyword;

/// A 2D transform: translate + rotate + uniform scale. Compound of the
/// `transform` properties the engine composites with.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transform {
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
    pub rotate_deg: f32,
}

impl Transform {
    pub fn is_identity(&self) -> bool {
        self.tx == 0.0 && self.ty == 0.0 && self.scale == 1.0 && self.rotate_deg == 0.0
    }
}

/// `animation-iteration-count`: a count or `infinite`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum IterationCount {
    #[default]
    One,
    Number(f32),
    Infinite,
}

impl IterationCount {
    pub fn is_infinite(&self) -> bool {
        matches!(self, IterationCount::Infinite)
    }
}

/// `animation-direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationDirection {
    #[default]
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

/// `animation-fill-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationFill {
    #[default]
    None,
    Forwards,
    Backwards,
    Both,
}

/// One comma-separated entry of the `animation` shorthand.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationSpec {
    pub name: String,
    pub duration_s: f32,
    pub delay_s: f32,
    pub iteration: IterationCount,
    pub direction: AnimationDirection,
    pub fill: AnimationFill,
    pub easing: crate::atr::EasingKeyword,
}

/// One comma-separated entry of the `transition` shorthand.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionSpec {
    /// Property name the transition applies to (`all` for everything).
    pub property: String,
    pub duration_s: f32,
    pub delay_s: f32,
    pub easing: crate::atr::EasingKeyword,
}

/// `container-type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContainerType {
    #[default]
    Normal,
    InlineSize,
    Size,
}

impl Default for AnimationSpec {
    fn default() -> Self {
        AnimationSpec {
            name: String::new(),
            duration_s: 0.0,
            delay_s: 0.0,
            iteration: IterationCount::One,
            direction: AnimationDirection::Normal,
            fill: AnimationFill::None,
            easing: EasingKeyword::Ease,
        }
    }
}

impl Default for TransitionSpec {
    fn default() -> Self {
        TransitionSpec {
            property: String::new(),
            duration_s: 0.0,
            delay_s: 0.0,
            easing: EasingKeyword::Ease,
        }
    }
}
