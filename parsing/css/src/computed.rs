//! Computed style: per-node resolved CSS values after cascade + inheritance.

use crate::atr::DeviceEnv;
use crate::values::*;

/// Fully resolved style for one node, in the property subset the v1
/// layout + rendering engines consume. Unrecognized properties are ignored
/// (see docs/ROADMAP.md for the supported-property matrix).
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    pub display: Display,
    pub color: Rgba,
    pub background_color: Rgba,
    pub background_gradient: Option<crate::values::Gradient>,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: FontStyle,
    pub font_family: Option<String>,
    pub line_height: LineHeight,
    pub text_align: TextAlign,
    pub text_transform: crate::values::TextTransform,
    pub text_underline: bool,
    pub text_line_through: bool,
    pub opacity: f32,
    pub margin: Edges<AutoPx>,
    pub padding: Edges<Len>,
    pub border_width: Edges<Len>,
    pub border_color: Rgba,
    pub border_radius: f32,
    pub width: AutoPx,
    pub height: AutoPx,
    /// `min-width` / `min-height`: `auto` (default) = content-based minimum;
    /// an explicit 0 lets flex/grid items shrink below their content size,
    /// exactly the `min-width: 0` idiom real skins depend on.
    pub min_width: AutoPx,
    pub min_height: AutoPx,
    pub max_width: AutoPx,
    pub overflow: OverflowKeyword,
    /// `visibility`: hidden boxes keep space but skip painting (children
    /// may override back to visible — checked per node at paint time).
    pub visibility: Visibility,
    pub position: Position,
    /// `float: left/right` — box is removed from normal flow (layout).
    pub float: FloatSide,
    /// `clear: left/right/both` — push below matching floats.
    pub clear: ClearSide,
    pub z_index: ZIndex,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub flex: FlexBox,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub row_gap: Len,
    pub column_gap: Len,
    // ---- CSS grid ---------------------------------------------------------
    /// `grid-template-columns` / `grid-template-rows` track lists.
    pub grid_template_columns: Vec<GridTrackSize>,
    pub grid_template_rows: Vec<GridTrackSize>,
    /// `grid-auto-columns` / `grid-auto-rows` (implicit track sizes).
    pub grid_auto_columns: Vec<GridTrackSize>,
    pub grid_auto_rows: Vec<GridTrackSize>,
    /// `grid-auto-flow`: row (default) or column, with optional dense packing.
    pub grid_auto_flow_column: bool,
    pub grid_auto_flow_dense: bool,
    /// `grid-column: <start> / <end>` and `grid-row: <start> / <end>`.
    pub grid_column: (GridLineSpec, GridLineSpec),
    pub grid_row: (GridLineSpec, GridLineSpec),
    pub white_space: WhiteSpace,
    // ---- v0.2 additions -------------------------------------------------
    /// `top/right/bottom/left` inset properties (positioning).
    pub insets: Edges<AutoPx>,
    /// `transform` (translate/rotate/scale) used by layout + compositor.
    pub transform: Transform,
    /// `animation` shorthand (first entry; multi-name lists documented).
    pub animation: Option<AnimationSpec>,
    /// `transition` shorthand entries.
    pub transitions: Vec<TransitionSpec>,
    /// `container-type` for container queries.
    pub container_type: ContainerType,
    /// CSS custom properties (`--*`) registered on this element.
    pub custom: std::collections::HashMap<String, String>,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            display: Display::Inline,
            color: [0, 0, 0, 255],
            background_color: TRANSPARENT,
            background_gradient: None,
            font_size: 16.0,
            font_weight: 400,
            font_style: FontStyle::Normal,
            font_family: None,
            line_height: LineHeight::Normal,
            text_align: TextAlign::Start,
            text_transform: crate::values::TextTransform::None,
            text_underline: false,
            text_line_through: false,
            opacity: 1.0,
            margin: Edges::splat(AutoPx::Len(Len::Px(0.0))),
            padding: Edges::splat(Len::Px(0.0)),
            border_width: Edges::splat(Len::Px(0.0)),
            border_color: [0, 0, 0, 255],
            border_radius: 0.0,
            width: AutoPx::Auto,
            height: AutoPx::Auto,
            min_width: AutoPx::Auto,
            min_height: AutoPx::Auto,
            max_width: AutoPx::Auto,
            overflow: OverflowKeyword::Visible,
            visibility: Visibility::Visible,
            position: Position::Static,
            float: FloatSide::None,
            clear: ClearSide::None,
            z_index: ZIndex::Auto,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex: FlexBox { grow: 0.0, shrink: 1.0 },
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::Stretch,
            row_gap: Len::Px(0.0),
            column_gap: Len::Px(0.0),
            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
            grid_auto_columns: Vec::new(),
            grid_auto_rows: Vec::new(),
            grid_auto_flow_column: false,
            grid_auto_flow_dense: false,
            grid_column: (GridLineSpec::Auto, GridLineSpec::Auto),
            grid_row: (GridLineSpec::Auto, GridLineSpec::Auto),
            white_space: WhiteSpace::Normal,
            insets: Edges::splat(AutoPx::Auto),
            transform: Transform::default(),
            animation: None,
            transitions: Vec::new(),
            container_type: ContainerType::Normal,
            custom: std::collections::HashMap::new(),
        }
    }
}

impl ComputedStyle {
    /// Resolve a unitless/absolute line height into pixels for this style.
    /// Zero line-heights (CSS resets) clamp to the normal single-space
    /// value — shaping engines reject 0.
    pub fn line_height_px(&self) -> f32 {
        let px = match self.line_height {
            LineHeight::Normal => self.font_size * 1.2,
            LineHeight::Number(n) => self.font_size * n,
            LineHeight::Px(px) => px,
        };
        if px <= 0.0 {
            self.font_size * 1.2
        } else {
            px
        }
    }

    /// Inheritable properties flow from `parent` into a fresh style.
    /// Custom properties are inherited wholesale (spec behaviour).
    pub fn inherit_from(parent: &ComputedStyle) -> Self {
        ComputedStyle {
            display: Display::Inline,
            color: parent.color,
            background_color: TRANSPARENT,
            background_gradient: None,
            font_size: parent.font_size,
            font_weight: parent.font_weight,
            font_style: parent.font_style,
            font_family: parent.font_family.clone(),
            line_height: parent.line_height,
            text_align: parent.text_align,
            text_transform: parent.text_transform,
            text_underline: false,
            text_line_through: false,
            opacity: 1.0,
            visibility: parent.visibility,
            custom: parent.custom.clone(),
            ..ComputedStyle::default()
        }
    }
}

/// Context needed to resolve font-relative and viewport-relative units.
#[derive(Debug, Clone, Copy)]
pub struct CascadeCtx {
    pub root_font_size: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Device environment for `@media` evaluation.
    pub device: DeviceEnv,
}

impl Default for CascadeCtx {
    fn default() -> Self {
        CascadeCtx {
            root_font_size: 16.0,
            viewport_width: 1280.0,
            viewport_height: 720.0,
            device: DeviceEnv {
                viewport_width: 1280.0,
                viewport_height: 720.0,
                ..DeviceEnv::default()
            },
        }
    }
}

pub(crate) struct LengthContext {
    pub font_size: f32,
    pub root_font_size: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
}

impl From<&CascadeCtx> for LengthContext {
    fn from(c: &CascadeCtx) -> Self {
        LengthContext {
            font_size: c.root_font_size,
            root_font_size: c.root_font_size,
            viewport_width: c.viewport_width,
            viewport_height: c.viewport_height,
        }
    }
}

/// Resolve a lightningcss length to px using the given context (font size for
/// em, root font size for rem, viewport for vw/vh).
pub(crate) fn length_to_px(
    value: &lightningcss::values::length::LengthValue,
    ctx: &LengthContext,
) -> Option<f32> {
    use lightningcss::values::length::LengthValue as LV;
    match value {
        LV::Px(v) => Some(*v),
        LV::Em(v) => Some(*v * ctx.font_size),
        LV::Rem(v) => Some(*v * ctx.root_font_size),
        LV::Vw(v) => Some(*v * ctx.viewport_width / 100.0),
        LV::Vh(v) => Some(*v * ctx.viewport_height / 100.0),
        other => other.to_px(),
    }
}

/// Resolve a `<length-percentage>` either to px or to a symbolic percentage.
pub(crate) fn length_percentage_to_len(
    lp: &lightningcss::values::length::LengthPercentage,
    ctx: &LengthContext,
) -> Option<Len> {
    use lightningcss::values::percentage::DimensionPercentage as DP;
    match lp {
        DP::Dimension(lv) => length_to_px(lv, ctx).map(Len::Px),
        DP::Percentage(p) => Some(Len::Percent(p.0)),
        DP::Calc(_) => None,
    }
}

/// Resolve a `<length-percentage | auto>`.
pub(crate) fn length_percentage_auto(
    lpa: &lightningcss::values::length::LengthPercentageOrAuto,
    ctx: &LengthContext,
) -> Option<AutoPx> {
    use lightningcss::values::length::LengthPercentageOrAuto as LPA;
    match lpa {
        LPA::Auto => Some(AutoPx::Auto),
        LPA::LengthPercentage(lp) => length_percentage_to_len(lp, ctx).map(AutoPx::Len),
    }
}
