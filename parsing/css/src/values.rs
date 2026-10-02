//! Owned value types shared between the cascade, layout and rendering.

/// A resolved horizontal/vertical length: absolute pixels or a percentage
/// (left symbolic so the layout engine can resolve it against the used
/// available space, exactly like the CSS layout algorithms expect).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Len {
    Px(f32),
    Percent(f32),
}

/// A length or the `auto` keyword.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoPx {
    Auto,
    Len(Len),
}

/// Per-edge box values (top, right, bottom, left).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    /// Same value on all four edges.
    pub fn splat(v: T) -> Self {
        Edges { top: v, right: v, bottom: v, left: v }
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
    Flex,
    None,
    /// CSS table boxes are laid out as anonymous flex structures
    /// (table → column flex, row → row flex, cell → flex item).
    Table,
    TableRow,
    TableCell,
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

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FlexBox {
    pub grow: f32,
    pub shrink: f32,
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
