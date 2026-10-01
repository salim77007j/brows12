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
