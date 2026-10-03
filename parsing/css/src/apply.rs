//! Application of lightningcss `Property` values to [`ComputedStyle`].

use crate::atr::EasingKeyword;
use crate::computed::*;
use crate::values::*;
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::display::Display as LcDisplay;
use lightningcss::properties::font::{
    AbsoluteFontSize, AbsoluteFontWeight, FontSize, FontWeight, RelativeFontSize,
};
use lightningcss::properties::position::{Position as LcPosition, ZIndex as LcZIndex};
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::text::{TextAlign as LcTextAlign, WhiteSpace as LcWhiteSpace};
use lightningcss::properties::Property;
use lightningcss::values::color::CssColor;

/// Apply one declaration onto `s`.
pub(crate) fn apply_property(
    s: &mut ComputedStyle,
    prop: &Property<'static>,
    parent: Option<&ComputedStyle>,
    ctx: &CascadeCtx,
) {
    let lctx = LengthContext {
        font_size: s.font_size,
        root_font_size: ctx.root_font_size,
        viewport_width: ctx.viewport_width,
        viewport_height: ctx.viewport_height,
    };
    match prop {
        // Author-defined `--*` declarations (including the `float`/`clear`
        // rewrites from `stylesheet::rewrite_float_decls`) land in the
        // element's custom map; the cascade resolves `--brows-float` /
        // `--brows-clear` into typed fields afterwards.
        Property::Custom(cp) => {
            let name = match &cp.name {
                lightningcss::properties::custom::CustomPropertyName::Custom(d) => {
                    d.0.to_string()
                }
                lightningcss::properties::custom::CustomPropertyName::Unknown(i) => {
                    i.0.to_string()
                }
            };
            if name.starts_with("--brows-") {
                // Reserved values are simple keywords (left/right/none/both);
                // take the first ident token from the raw token list.
                for tov in cp.value.0.iter() {
                    if let lightningcss::properties::custom::TokenOrValue::Token(t) = tov {
                        if let lightningcss::properties::custom::Token::Ident(word) = t {
                            s.custom.insert(name, word.to_string());
                        }
                        break;
                    }
                }
            }
        }
        Property::BackgroundColor(c) => {
            if let Some(rgba) = color(c, s.color) {
                s.background_color = rgba;
            }
        }
        Property::Background(bg_list) => {
            // Shorthand: first layer's color + first gradient image.
            if let Some(first) = bg_list.first() {
                if let Some(rgba) = color(&first.color, s.color) {
                    s.background_color = rgba;
                }
                if let Some(g) = gradient_of(&first.image) {
                    s.background_gradient = Some(g);
                }
            }
        }
        Property::BackgroundImage(images) => {
            for img in images.iter() {
                if let Some(g) = gradient_of(img) {
                    s.background_gradient = Some(g);
                    break;
                }
            }
        }
        Property::Color(c) => {
            if let Some(rgba) = color(c, s.color) {
                s.color = rgba;
            }
        }
        Property::Display(d) => s.display = map_display(d),
        Property::FontSize(fs) => {
            let parent_size = parent.map(|p| p.font_size).unwrap_or(ctx.root_font_size);
            // em/percentage font sizes resolve against the PARENT's computed
            // font size (CSS 2.1 §6.1) — never against this element's
            // already-cascaded value, which would compound across every
            // matched rule (the "giant heading" bug on skins that restate
            // `h2 { font-size: 1.5em }` after the UA sheet).
            let mut lctx_fs = lctx;
            lctx_fs.font_size = parent_size;
            let resolved = match fs {
                FontSize::Length(lp) => {
                    crate::computed::length_percentage_to_len(lp, &lctx_fs)
                        .map(|len| len.resolve(parent_size))
                }
                FontSize::Absolute(kw) => Some(absolute_font_size(*kw)),
                FontSize::Relative(r) => Some(match r {
                    RelativeFontSize::Larger => parent_size * 1.2,
                    RelativeFontSize::Smaller => parent_size / 1.2,
                }),
            };
            if let Some(px) = resolved {
                s.font_size = px.max(1.0);
            }
        }
        Property::FontWeight(w) => {
            let base = parent.map(|p| p.font_weight).unwrap_or(400);
            s.font_weight = match w {
                FontWeight::Absolute(a) => match a {
                    AbsoluteFontWeight::Weight(n) => (*n as u16).clamp(1, 1000),
                    AbsoluteFontWeight::Normal => 400,
                    AbsoluteFontWeight::Bold => 700,
                },
                FontWeight::Bolder => ((base as f32) * 1.5).round() as u16,
                FontWeight::Lighter => ((base as f32) / 1.5).round() as u16,
            };
        }
        Property::FontFamily(families) => {
            for fam in families {
                if let Some(name) = family_name(fam) {
                    s.font_family = Some(name);
                    break;
                }
            }
        }
        Property::FontStyle(st) => {
            use lightningcss::properties::font::FontStyle as LcFS;
            s.font_style = match st {
                LcFS::Italic => FontStyle::Italic,
                _ => FontStyle::Normal,
            };
        }
        Property::LineHeight(lh) => {
            use lightningcss::properties::font::LineHeight as LcLH;
            s.line_height = match lh {
                LcLH::Normal => LineHeight::Normal,
                LcLH::Number(n) => LineHeight::Number(*n),
                LcLH::Length(lp) => crate::computed::length_percentage_to_len(lp, &lctx)
                    .map(|len| LineHeight::Px(len.resolve(s.font_size)))
                    .unwrap_or(LineHeight::Normal),
            };
        }
        // Logical margins/paddings/insets map onto the physical axes
        // (horizontal-tb writing mode only — documented).
        Property::MarginInline(m) => {
            let (start, end) = (&m.inline_start, &m.inline_end);
            s.margin.left = logical_lpa(start, &lctx).unwrap_or_else(|| s.margin.left.clone());
            s.margin.right = logical_lpa(end, &lctx).unwrap_or_else(|| s.margin.right.clone());
        }
        Property::MarginInlineStart(v) => {
            if let Some(x) = logical_lpa(v, &lctx) {
                s.margin.left = x;
            }
        }
        Property::MarginInlineEnd(v) => {
            if let Some(x) = logical_lpa(v, &lctx) {
                s.margin.right = x;
            }
        }
        Property::PaddingInline(m) => {
            if let Some(AutoPx::Len(len)) = logical_lpa(&m.inline_start, &lctx) {
                s.padding.left = len;
            }
            if let Some(AutoPx::Len(len)) = logical_lpa(&m.inline_end, &lctx) {
                s.padding.right = len;
            }
        }
        Property::PaddingInlineStart(v) => {
            if let Some(AutoPx::Len(len)) = logical_lpa(v, &lctx) {
                s.padding.left = len;
            }
        }
        Property::PaddingInlineEnd(v) => {
            if let Some(AutoPx::Len(len)) = logical_lpa(v, &lctx) {
                s.padding.right = len;
            }
        }
        Property::InsetInline(m) => {
            if let Some(x) = lpa_auto(&m.inline_start, &lctx) {
                s.insets.left = x;
            }
            if let Some(x) = lpa_auto(&m.inline_end, &lctx) {
                s.insets.right = x;
            }
        }
        Property::InsetInlineStart(v) => {
            if let Some(x) = lpa_auto(v, &lctx) {
                s.insets.left = x;
            }
        }
        Property::InsetInlineEnd(v) => {
            if let Some(x) = lpa_auto(v, &lctx) {
                s.insets.right = x;
            }
        }
        Property::Margin(m) => {
            if let (Some(t), Some(r), Some(b), Some(l)) = (
                length_percentage_auto(&m.top, &lctx),
                length_percentage_auto(&m.right, &lctx),
                length_percentage_auto(&m.bottom, &lctx),
                length_percentage_auto(&m.left, &lctx),
            ) {
                s.margin = Edges { top: t, right: r, bottom: b, left: l };
            }
        }
        Property::Padding(p) => {
            let f = |v: &lightningcss::values::length::LengthPercentageOrAuto| {
                match crate::computed::length_percentage_auto(v, &lctx) {
                    Some(AutoPx::Len(len)) => len,
                    _ => Len::Px(0.0),
                }
            };
            s.padding = Edges {
                top: f(&p.top),
                right: f(&p.right),
                bottom: f(&p.bottom),
                left: f(&p.left),
            };
        }
        Property::MarginTop(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.margin.top = a;
            }
        }
        Property::MarginRight(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.margin.right = a;
            }
        }
        Property::MarginBottom(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.margin.bottom = a;
            }
        }
        Property::MarginLeft(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.margin.left = a;
            }
        }
        Property::PaddingTop(v) => {
            s.padding.top = length_percentage_auto(v, &lctx)
                .map(|a| match a {
                    AutoPx::Len(l) => l,
                    AutoPx::Auto => Len::Px(0.0),
                })
                .unwrap_or(Len::Px(0.0));
        }
        Property::PaddingRight(v) => {
            s.padding.right = length_percentage_auto(v, &lctx)
                .map(|a| match a {
                    AutoPx::Len(l) => l,
                    AutoPx::Auto => Len::Px(0.0),
                })
                .unwrap_or(Len::Px(0.0));
        }
        Property::PaddingBottom(v) => {
            s.padding.bottom = length_percentage_auto(v, &lctx)
                .map(|a| match a {
                    AutoPx::Len(l) => l,
                    AutoPx::Auto => Len::Px(0.0),
                })
                .unwrap_or(Len::Px(0.0));
        }
        Property::PaddingLeft(v) => {
            s.padding.left = length_percentage_auto(v, &lctx)
                .map(|a| match a {
                    AutoPx::Len(l) => l,
                    AutoPx::Auto => Len::Px(0.0),
                })
                .unwrap_or(Len::Px(0.0));
        }
        Property::Width(v) => {
            if let Some(a) = size(v, &lctx) {
                s.width = a;
            }
        }
        Property::Height(v) => {
            if let Some(a) = size(v, &lctx) {
                s.height = a;
            }
        }
        Property::MaxWidth(v) => {
            if let Some(a) = max_size(v, &lctx) {
                s.max_width = a;
            }
        }
        Property::MinWidth(v) => {
            if let Some(a) = min_size(v, &lctx) {
                s.min_width = a;
            }
        }
        Property::MinHeight(v) => {
            if let Some(a) = min_size(v, &lctx) {
                s.min_height = a;
            }
        }
        // `border` shorthand: width + style + color on all four sides.
        Property::Border(b) => {
            let w = side_width(&b.width, &lctx);
            let w = style_width(&w, &b.style);
            s.border_width = Edges::splat(w);
            if let Some(rgba) = color(&b.color, s.color) {
                s.border_color = rgba;
            }
        }
        Property::BorderWidth(bw) => {
            s.border_width = Edges {
                top: side_width(&bw.top, &lctx),
                right: side_width(&bw.right, &lctx),
                bottom: side_width(&bw.bottom, &lctx),
                left: side_width(&bw.left, &lctx),
            };
        }
        Property::BorderTopWidth(v) => s.border_width.top = side_width(v, &lctx),
        Property::BorderRightWidth(v) => s.border_width.right = side_width(v, &lctx),
        Property::BorderBottomWidth(v) => s.border_width.bottom = side_width(v, &lctx),
        Property::BorderLeftWidth(v) => s.border_width.left = side_width(v, &lctx),
        Property::BorderTopStyle(v) => s.border_width.top = style_width(&s.border_width.top, v),
        Property::BorderRightStyle(v) => {
            s.border_width.right = style_width(&s.border_width.right, v)
        }
        Property::BorderBottomStyle(v) => {
            s.border_width.bottom = style_width(&s.border_width.bottom, v)
        }
        Property::BorderLeftStyle(v) => s.border_width.left = style_width(&s.border_width.left, v),
        Property::BorderColor(c) => {
            if let Some(rgba) = color(&c.top, s.border_color) {
                s.border_color = rgba;
            }
        }
        Property::BorderRadius(br, _) => {
            let avg = |sp: &lightningcss::values::size::Size2D<
                lightningcss::values::length::LengthPercentage,
            >| {
                let a = crate::computed::length_percentage_to_len(&sp.0, &lctx);
                let b = crate::computed::length_percentage_to_len(&sp.1, &lctx);
                match (a, b) {
                    (Some(Len::Px(x)), Some(Len::Px(y))) => (x + y) / 2.0,
                    _ => 0.0,
                }
            };
            let r = (avg(&br.top_left)
                + avg(&br.top_right)
                + avg(&br.bottom_right)
                + avg(&br.bottom_left))
                / 4.0;
            if r > 0.0 {
                s.border_radius = r;
            }
        }
        Property::Opacity(a) => {
            s.opacity = a.0.clamp(0.0, 1.0);
        }
        Property::TextAlign(t) => {
            s.text_align = match t {
                LcTextAlign::Left => TextAlign::Left,
                LcTextAlign::Right => TextAlign::Right,
                LcTextAlign::Center => TextAlign::Center,
                LcTextAlign::Justify | LcTextAlign::JustifyAll => TextAlign::Justify,
                _ => TextAlign::Start,
            };
        }
        Property::TextTransform(tt) => {
            s.text_transform = match tt.case {
                lightningcss::properties::text::TextTransformCase::Uppercase => {
                    crate::values::TextTransform::Uppercase
                }
                lightningcss::properties::text::TextTransformCase::Lowercase => {
                    crate::values::TextTransform::Lowercase
                }
                lightningcss::properties::text::TextTransformCase::Capitalize => {
                    crate::values::TextTransform::Capitalize
                }
                _ => crate::values::TextTransform::None,
            };
        }
        Property::WhiteSpace(w) => {
            s.white_space = match w {
                LcWhiteSpace::Pre | LcWhiteSpace::PreWrap | LcWhiteSpace::BreakSpaces => {
                    WhiteSpace::Pre
                }
                LcWhiteSpace::NoWrap => WhiteSpace::NoWrap,
                _ => WhiteSpace::Normal,
            };
        }
        Property::TextDecoration(td, _) => {
            s.text_underline =
                td.line.contains(lightningcss::properties::text::TextDecorationLine::Underline);
            s.text_line_through =
                td.line.contains(lightningcss::properties::text::TextDecorationLine::LineThrough);
        }
        Property::TextDecorationLine(line, _) => {
            s.text_underline =
                line.contains(lightningcss::properties::text::TextDecorationLine::Underline);
            s.text_line_through =
                line.contains(lightningcss::properties::text::TextDecorationLine::LineThrough);
        }
        Property::FlexDirection(d, _) => {
            s.flex_direction = match d {
                lightningcss::properties::flex::FlexDirection::Row => FlexDirection::Row,
                lightningcss::properties::flex::FlexDirection::RowReverse => {
                    FlexDirection::RowReverse
                }
                lightningcss::properties::flex::FlexDirection::Column => FlexDirection::Column,
                lightningcss::properties::flex::FlexDirection::ColumnReverse => {
                    FlexDirection::ColumnReverse
                }
            };
        }
        Property::FlexWrap(w, _) => {
            s.flex_wrap = match w {
                lightningcss::properties::flex::FlexWrap::Wrap => FlexWrap::Wrap,
                lightningcss::properties::flex::FlexWrap::WrapReverse => FlexWrap::WrapReverse,
                _ => FlexWrap::NoWrap,
            };
        }
        Property::Flex(f, _) => {
            s.flex.grow = f.grow;
            s.flex.shrink = f.shrink;
        }
        Property::FlexGrow(g, _) => s.flex.grow = *g,
        Property::JustifyContent(j, _) => {
            use lightningcss::properties::align::{
                ContentDistribution, ContentPosition, JustifyContent as LcJ,
            };
            s.justify_content = match j {
                LcJ::ContentDistribution(d) => match d {
                    ContentDistribution::SpaceBetween => JustifyContent::SpaceBetween,
                    ContentDistribution::SpaceAround => JustifyContent::SpaceAround,
                    ContentDistribution::SpaceEvenly => JustifyContent::SpaceEvenly,
                    _ => JustifyContent::FlexStart,
                },
                LcJ::ContentPosition { value: position, .. } => match position {
                    ContentPosition::Center => JustifyContent::Center,
                    ContentPosition::Start => JustifyContent::Start,
                    ContentPosition::End => JustifyContent::End,
                    ContentPosition::FlexStart => JustifyContent::FlexStart,
                    ContentPosition::FlexEnd => JustifyContent::FlexEnd,
                },
                _ => JustifyContent::FlexStart,
            };
        }
        Property::AlignItems(a, _) => {
            use lightningcss::properties::align::{AlignItems as LcA, SelfPosition};
            s.align_items = match a {
                LcA::Stretch => AlignItems::Stretch,
                LcA::BaselinePosition(_) => AlignItems::Baseline,
                LcA::SelfPosition { value: position, .. } => match position {
                    SelfPosition::Center => AlignItems::Center,
                    SelfPosition::Start => AlignItems::Start,
                    SelfPosition::End => AlignItems::End,
                    SelfPosition::FlexStart => AlignItems::FlexStart,
                    SelfPosition::FlexEnd => AlignItems::FlexEnd,
                    _ => AlignItems::Stretch,
                },
                _ => AlignItems::Stretch,
            };
        }
        Property::Gap(g) => {
            s.row_gap = gap_value(&g.row, &lctx);
            s.column_gap = gap_value(&g.column, &lctx);
        }
        Property::RowGap(g) => s.row_gap = gap_value(g, &lctx),
        Property::ColumnGap(g) => s.column_gap = gap_value(g, &lctx),

        // ---- CSS grid (css-grid-2) ------------------------------------
        Property::GridTemplateColumns(ts) => {
            s.grid_template_columns = track_sizing(ts, &lctx);
        }
        Property::GridTemplateRows(ts) => {
            s.grid_template_rows = track_sizing(ts, &lctx);
        }
        Property::GridAutoColumns(list) => {
            s.grid_auto_columns = track_size_list(&list.0, &lctx);
        }
        Property::GridAutoRows(list) => {
            s.grid_auto_rows = track_size_list(&list.0, &lctx);
        }
        Property::GridAutoFlow(flow) => {
            s.grid_auto_flow_column = flow.contains(lightningcss::properties::grid::GridAutoFlow::Column);
            s.grid_auto_flow_dense = flow.contains(lightningcss::properties::grid::GridAutoFlow::Dense);
        }
        Property::GridColumn(gc) => {
            s.grid_column = (grid_line(&gc.start), grid_line(&gc.end));
        }
        Property::GridRow(gr) => {
            s.grid_row = (grid_line(&gr.start), grid_line(&gr.end));
        }
        Property::GridColumnStart(gl) => s.grid_column.0 = grid_line(gl),
        Property::GridColumnEnd(gl) => s.grid_column.1 = grid_line(gl),
        Property::GridRowStart(gl) => s.grid_row.0 = grid_line(gl),
        Property::GridRowEnd(gl) => s.grid_row.1 = grid_line(gl),
        Property::Overflow(o) => s.overflow = map_overflow(o.y),
        Property::OverflowY(o) => s.overflow = map_overflow(*o),
        Property::Visibility(v) => {
            use lightningcss::properties::display::Visibility as LV;
            s.visibility = match v {
                LV::Visible => Visibility::Visible,
                LV::Hidden => Visibility::Hidden,
                LV::Collapse => Visibility::Collapse,
            };
        }
        Property::Position(p) => {
            s.position = match p {
                LcPosition::Static => Position::Static,
                LcPosition::Relative => Position::Relative,
                LcPosition::Absolute => Position::Absolute,
                LcPosition::Fixed => Position::Fixed,
                LcPosition::Sticky(_) => Position::Sticky,
            };
        }
        Property::ZIndex(z) => {
            s.z_index = match z {
                LcZIndex::Auto => ZIndex::Auto,
                LcZIndex::Integer(i) => ZIndex::Number(*i),
            };
        }
        // ---- v0.2: positioning insets ---------------------------------
        Property::Top(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.insets.top = a;
            }
        }
        Property::Right(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.insets.right = a;
            }
        }
        Property::Bottom(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.insets.bottom = a;
            }
        }
        Property::Left(v) => {
            if let Some(a) = length_percentage_auto(v, &lctx) {
                s.insets.left = a;
            }
        }
        // ---- v0.2: transforms ------------------------------------------
        Property::Transform(list, _) => {
            s.transform = transform_of_list(list, s.transform, &lctx);
        }
        Property::Translate(t) => {
            use lightningcss::properties::transform::Translate;
            if let Translate::XYZ { x, y, .. } = t {
                s.transform.tx = lp_px(x, &lctx);
                s.transform.ty = lp_px(y, &lctx);
            }
        }
        Property::Rotate(r) => {
            use lightningcss::properties::transform::Rotate;
            if let Rotate::XYZ { angle, .. } = r {
                s.transform.rotate_deg = angle.to_degrees();
            }
        }
        Property::Scale(sc) => {
            use lightningcss::properties::transform::Scale;
            use lightningcss::values::percentage::NumberOrPercentage as NOP;
            if let Scale::XYZ { x, .. } = sc {
                s.transform.scale = match x {
                    NOP::Number(n) => *n,
                    NOP::Percentage(p) => p.0,
                };
            }
        }
        // ---- v0.2: animation + transition ------------------------------
        Property::Animation(anims, _) => {
            if let Some(a) = anims.first() {
                s.animation = Some(map_animation(a));
            }
        }
        Property::AnimationName(names, _) => {
            use lightningcss::properties::animation::AnimationName as AN;
            for n in names {
                if let AN::Ident(id) = n {
                    let name = id.0.to_string();
                    let spec = s.animation.take().unwrap_or_default();
                    s.animation = Some(AnimationSpec { name, ..spec });
                    break;
                }
            }
        }
        Property::AnimationDuration(times, _) => {
            if let Some(t) = times.first() {
                let spec = s.animation.take().unwrap_or_default();
                s.animation = Some(AnimationSpec { duration_s: t.to_ms() / 1000.0, ..spec });
            }
        }
        Property::AnimationDelay(times, _) => {
            if let Some(t) = times.first() {
                let spec = s.animation.take().unwrap_or_default();
                s.animation = Some(AnimationSpec { delay_s: t.to_ms() / 1000.0, ..spec });
            }
        }
        Property::AnimationIterationCount(counts, _) => {
            use lightningcss::properties::animation::AnimationIterationCount as LIC;
            if let Some(c) = counts.first() {
                let it = match c {
                    LIC::Number(n) => IterationCount::Number(*n),
                    LIC::Infinite => IterationCount::Infinite,
                };
                let spec = s.animation.take().unwrap_or_default();
                s.animation = Some(AnimationSpec { iteration: it, ..spec });
            }
        }
        Property::AnimationDirection(dirs, _) => {
            use lightningcss::properties::animation::AnimationDirection as LAD;
            if let Some(d) = dirs.first() {
                let dir = match d {
                    LAD::Reverse => AnimationDirection::Reverse,
                    LAD::Alternate => AnimationDirection::Alternate,
                    LAD::AlternateReverse => AnimationDirection::AlternateReverse,
                    _ => AnimationDirection::Normal,
                };
                let spec = s.animation.take().unwrap_or_default();
                s.animation = Some(AnimationSpec { direction: dir, ..spec });
            }
        }
        Property::AnimationFillMode(modes, _) => {
            use lightningcss::properties::animation::AnimationFillMode as LAF;
            if let Some(m) = modes.first() {
                let fill = match m {
                    LAF::Forwards => AnimationFill::Forwards,
                    LAF::Backwards => AnimationFill::Backwards,
                    LAF::Both => AnimationFill::Both,
                    _ => AnimationFill::None,
                };
                let spec = s.animation.take().unwrap_or_default();
                s.animation = Some(AnimationSpec { fill, ..spec });
            }
        }
        Property::AnimationTimingFunction(fns, _) => {
            if let Some(f) = fns.first() {
                if let Some(e) = crate::atr::timing_to_easing(f) {
                    let spec = s.animation.take().unwrap_or_default();
                    s.animation = Some(AnimationSpec { easing: e, ..spec });
                }
            }
        }
        Property::Transition(transitions, _) => {
            s.transitions = transitions
                .iter()
                .map(|t| TransitionSpec {
                    property: crate::atr::to_css_string(&t.property)
                        .map(|p| p.to_ascii_lowercase())
                        .unwrap_or_default(),
                    duration_s: t.duration.to_ms() / 1000.0,
                    delay_s: t.delay.to_ms() / 1000.0,
                    easing: crate::atr::timing_to_easing(&t.timing_function)
                        .unwrap_or(EasingKeyword::Ease),
                })
                .collect();
        }
        Property::ContainerType(ct) => {
            use lightningcss::properties::contain::ContainerType as LCT;
            s.container_type = match ct {
                LCT::InlineSize => ContainerType::InlineSize,
                LCT::Size => ContainerType::Size,
                _ => ContainerType::Normal,
            };
        }
        _ => {
            // Unrecognized property: intentionally ignored in v1.
        }
    }
}

fn map_animation(a: &lightningcss::properties::animation::Animation) -> AnimationSpec {
    use lightningcss::properties::animation::{
        AnimationDirection as LAD, AnimationFillMode as LAF, AnimationIterationCount as LIC,
        AnimationName as AN,
    };
    let name = match &a.name {
        AN::Ident(id) => id.0.to_string(),
        AN::String(s) => s.to_string(),
        _ => String::new(),
    };
    AnimationSpec {
        name,
        duration_s: a.duration.to_ms() / 1000.0,
        delay_s: a.delay.to_ms() / 1000.0,
        iteration: match &a.iteration_count {
            LIC::Number(n) => IterationCount::Number(*n),
            LIC::Infinite => IterationCount::Infinite,
        },
        direction: match &a.direction {
            LAD::Reverse => AnimationDirection::Reverse,
            LAD::Alternate => AnimationDirection::Alternate,
            LAD::AlternateReverse => AnimationDirection::AlternateReverse,
            _ => AnimationDirection::Normal,
        },
        fill: match &a.fill_mode {
            LAF::Forwards => AnimationFill::Forwards,
            LAF::Backwards => AnimationFill::Backwards,
            LAF::Both => AnimationFill::Both,
            _ => AnimationFill::None,
        },
        easing: crate::atr::timing_to_easing(&a.timing_function).unwrap_or(EasingKeyword::Ease),
    }
}

/// Resolve a LengthPercentage to px (percentages resolve to 0 without box info).
fn lp_px(lp: &lightningcss::values::length::LengthPercentage, lctx: &LengthContext) -> f32 {
    use lightningcss::values::percentage::DimensionPercentage as DP;
    match lp {
        DP::Dimension(l) => crate::computed::length_to_px(l, lctx).unwrap_or(0.0),
        DP::Percentage(_) | DP::Calc(_) => 0.0,
    }
}

fn transform_of_list(
    list: &lightningcss::properties::transform::TransformList,
    current: Transform,
    lctx: &LengthContext,
) -> Transform {
    use lightningcss::properties::transform::Transform as LT;
    let mut t = current;
    for item in &list.0 {
        match item {
            LT::Translate(x, y) => {
                t.tx += lp_px(x, lctx);
                t.ty += lp_px(y, lctx);
            }
            LT::TranslateX(x) => t.tx += lp_px(x, lctx),
            LT::TranslateY(y) => t.ty += lp_px(y, lctx),
            LT::TranslateZ(_) => {}
            LT::Translate3d(x, y, _) => {
                t.tx += lp_px(x, lctx);
                t.ty += lp_px(y, lctx);
            }
            LT::Scale(x, _) => t.scale *= nop_value(x),
            LT::ScaleX(x) => t.scale *= nop_value(x),
            LT::ScaleY(_) => {}
            LT::ScaleZ(_) => {}
            LT::Scale3d(x, _, _) => t.scale *= nop_value(x),
            LT::Rotate(a) => t.rotate_deg += a.to_degrees(),
            LT::RotateZ(a) => t.rotate_deg += a.to_degrees(),
            LT::RotateX(_) | LT::RotateY(_) | LT::Rotate3d(..) => {}
            LT::Matrix(m) => {
                t.tx += m.e;
                t.ty += m.f;
                t.scale *= m.a;
                t.rotate_deg += m.b.atan2(m.a).to_degrees();
            }
            _ => {}
        }
    }
    t
}

fn nop_value(n: &lightningcss::values::percentage::NumberOrPercentage) -> f32 {
    use lightningcss::values::percentage::NumberOrPercentage as NOP;
    match n {
        NOP::Number(n) => *n,
        NOP::Percentage(p) => p.0,
    }
}

fn color(c: &CssColor, inherited: Rgba) -> Option<Rgba> {
    crate::stylesheet::resolve_color(c, inherited)
}

/// Convert a lightningcss image into our gradient paint (gradients only).
fn gradient_of(img: &lightningcss::values::image::Image) -> Option<crate::values::Gradient> {
    use lightningcss::values::gradient::{Gradient as LcGradient, LineDirection};
    let (lc, repeating) = match img {
        lightningcss::values::image::Image::Gradient(boxed) => match boxed.as_ref() {
            LcGradient::Linear(l) => (l, false),
            LcGradient::RepeatingLinear(l) => (l, true),
            LcGradient::Radial(r) => {
                let stops = convert_stops(&r.items)?;
                return Some(crate::values::Gradient::Radial { stops });
            }
            _ => return None,
        },
        _ => return None,
    };
    let _ = repeating; // v1: repeating renders as non-repeating
    let angle_deg = match &lc.direction {
        LineDirection::Angle(a) => match a {
            lightningcss::values::angle::Angle::Deg(d) => *d,
            lightningcss::values::angle::Angle::Rad(r) => r * 180.0 / std::f32::consts::PI,
            lightningcss::values::angle::Angle::Grad(g) => g * 0.9,
            lightningcss::values::angle::Angle::Turn(t) => t * 360.0,
        },
        LineDirection::Horizontal(h) => match h {
            lightningcss::values::position::HorizontalPositionKeyword::Left => 270.0,
            lightningcss::values::position::HorizontalPositionKeyword::Right => 90.0,
        },
        LineDirection::Vertical(v) => match v {
            lightningcss::values::position::VerticalPositionKeyword::Top => 0.0,
            lightningcss::values::position::VerticalPositionKeyword::Bottom => 180.0,
        },
        LineDirection::Corner { horizontal, vertical } => {
            use lightningcss::values::position::{
                HorizontalPositionKeyword as H, VerticalPositionKeyword as V,
            };
            let hx = match horizontal {
                H::Left => -1.0,
                H::Right => 1.0,
            };
            let vy = match vertical {
                V::Top => -1.0,
                V::Bottom => 1.0,
            };
            // Corner direction: normalize the (hx, vy) diagonal to CSS angle.
            let deg = f32::atan2(hx, -vy).to_degrees();
            deg.rem_euclid(360.0)
        }
    };
    let stops = convert_stops(&lc.items)?;
    Some(crate::values::Gradient::Linear { angle_deg, stops })
}

fn convert_stops(
    items: &[lightningcss::values::gradient::GradientItem<
        lightningcss::values::length::LengthPercentage,
    >],
) -> Option<Vec<crate::values::GradientStop>> {
    use lightningcss::values::percentage::DimensionPercentage as DP;
    let mut stops = Vec::new();
    for item in items {
        if let lightningcss::values::gradient::GradientItem::ColorStop(cs) = item {
            let rgba = color(&cs.color, [0, 0, 0, 255])?;
            let position = match &cs.position {
                Some(DP::Percentage(p)) => Some(p.0),
                Some(DP::Dimension(_)) => None, // px positions: v1 approximates evenly
                Some(DP::Calc(_)) | None => None,
            };
            stops.push(crate::values::GradientStop { color: rgba, position });
        }
        // Hints are skipped (v1).
    }
    if stops.is_empty() {
        None
    } else {
        Some(stops)
    }
}

fn size(v: &Size, lctx: &LengthContext) -> Option<AutoPx> {
    match v {
        Size::Auto => Some(AutoPx::Auto),
        Size::LengthPercentage(lp) => {
            crate::computed::length_percentage_to_len(lp, lctx).map(AutoPx::Len)
        }
        _ => None,
    }
}

/// `min-width`/`min-height`: `auto` (content-based minimum) or a
/// length/percentage. An explicit 0 lets flex/grid items shrink below
/// their content size — the `min-width: 0` idiom real skins depend on.
fn min_size(v: &Size, lctx: &LengthContext) -> Option<AutoPx> {
    match v {
        Size::Auto | Size::MinContent(_) | Size::MaxContent(_) | Size::FitContent(_)
        | Size::FitContentFunction(_) | Size::Stretch(_) | Size::Contain => Some(AutoPx::Auto),
        Size::LengthPercentage(lp) => length_percentage_to_len(lp, lctx).map(AutoPx::Len),
    }
}

fn max_size(v: &MaxSize, lctx: &LengthContext) -> Option<AutoPx> {
    match v {
        MaxSize::None => Some(AutoPx::Auto),
        MaxSize::LengthPercentage(lp) => {
            crate::computed::length_percentage_to_len(lp, lctx).map(AutoPx::Len)
        }
        _ => None,
    }
}


/// `<length-percentage-or-auto>` for logical margins.
fn logical_lpa(
    v: &lightningcss::values::length::LengthPercentageOrAuto,
    lctx: &LengthContext,
) -> Option<AutoPx> {
    use lightningcss::values::length::LengthPercentageOrAuto as LPA;
    match v {
        LPA::Auto => Some(AutoPx::Auto),
        LPA::LengthPercentage(lp) => {
            crate::computed::length_percentage_to_len(lp, lctx).map(AutoPx::Len)
        }
    }
}

fn lpa_auto(
    v: &lightningcss::values::length::LengthPercentageOrAuto,
    lctx: &LengthContext,
) -> Option<AutoPx> {
    logical_lpa(v, lctx)
}

fn side_width(v: &BorderSideWidth, _lctx: &LengthContext) -> Len {
    match v {
        BorderSideWidth::Thin => Len::Px(1.0),
        BorderSideWidth::Medium => Len::Px(3.0),
        BorderSideWidth::Thick => Len::Px(5.0),
        BorderSideWidth::Length(l) => l.to_px().map(Len::Px).unwrap_or(Len::Px(0.0)),
    }
}

fn style_width(current: &Len, style: &LineStyle) -> Len {
    match style {
        LineStyle::None | LineStyle::Hidden => Len::Px(0.0),
        _ => {
            if matches!(current, Len::Px(0.0)) {
                Len::Px(3.0)
            } else {
                current.clone()
            }
        }
    }
}

fn gap_value(g: &lightningcss::properties::align::GapValue, lctx: &LengthContext) -> Len {
    use lightningcss::properties::align::GapValue as GV;
    match g {
        GV::Normal => Len::Px(0.0),
        GV::LengthPercentage(lp) => {
            crate::computed::length_percentage_to_len(lp, lctx).unwrap_or(Len::Px(0.0))
        }
    }
}

fn map_overflow(kw: lightningcss::properties::overflow::OverflowKeyword) -> OverflowKeyword {
    use lightningcss::properties::overflow::OverflowKeyword as K;
    match kw {
        K::Hidden => OverflowKeyword::Hidden,
        K::Scroll => OverflowKeyword::Scroll,
        K::Auto => OverflowKeyword::Auto,
        _ => OverflowKeyword::Visible,
    }
}

fn absolute_font_size(kw: AbsoluteFontSize) -> f32 {
    match kw {
        AbsoluteFontSize::XXSmall => 9.0,
        AbsoluteFontSize::XSmall => 10.0,
        AbsoluteFontSize::Small => 13.0,
        AbsoluteFontSize::Medium => 16.0,
        AbsoluteFontSize::Large => 18.0,
        AbsoluteFontSize::XLarge => 24.0,
        AbsoluteFontSize::XXLarge => 32.0,
        AbsoluteFontSize::XXXLarge => 48.0,
    }
}

fn family_name(f: &lightningcss::properties::font::FontFamily) -> Option<String> {
    use lightningcss::properties::font::{FontFamily as FF, GenericFontFamily};
    match f {
        FF::FamilyName(name) => family_name_string(name),
        FF::Generic(g) => match g {
            GenericFontFamily::Monospace | GenericFontFamily::UIMonospace => {
                Some("monospace".into())
            }
            GenericFontFamily::Serif | GenericFontFamily::UISerif => Some("serif".into()),
            _ => None,
        },
    }
}

fn family_name_string(name: &lightningcss::properties::font::FamilyName) -> Option<String> {
    use lightningcss::printer::{Printer, PrinterOptions};
    use lightningcss::traits::ToCss;
    let mut out = String::new();
    let mut printer = Printer::new(&mut out, PrinterOptions { minify: true, ..Default::default() });
    name.to_css(&mut printer).ok()?;
    let trimmed = out.trim_matches('"').trim_matches('\'').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Map a lightningcss track sizing value (template) onto our symbolic
/// `GridTrackSize` list, expanding `repeat(N, ...)` counts.
fn track_sizing(
    ts: &lightningcss::properties::grid::TrackSizing<'static>,
    lctx: &LengthContext,
) -> Vec<crate::values::GridTrackSize> {
    use lightningcss::properties::grid::{TrackListItem, TrackSizing};
    let mut out = Vec::new();
    if let TrackSizing::TrackList(list) = ts {
        for item in &list.items {
            match item {
                TrackListItem::TrackSize(size) => {
                    out.push(track_size(size, lctx));
                }
                TrackListItem::TrackRepeat(rep) => {
                    let count = match rep.count {
                        lightningcss::properties::grid::RepeatCount::Number(n) => {
                            n.clamp(0, 64) as usize
                        }
                        // auto-fill/auto-fit depend on the available space;
                        // approximate with one copy (documented limitation).
                        _ => 1,
                    };
                    for _ in 0..count {
                        for t in &rep.track_sizes {
                            out.push(track_size(t, lctx));
                            if out.len() >= 64 {
                                return out;
                            }
                        }
                    }
                }
            }
            if out.len() >= 64 {
                break;
            }
        }
    }
    out
}

/// Map a `grid-auto-columns`/`rows` track size list.
fn track_size_list(
    list: &[lightningcss::properties::grid::TrackSize],
    lctx: &LengthContext,
) -> Vec<crate::values::GridTrackSize> {
    list.iter().map(|t| track_size(t, lctx)).collect()
}

/// One `<track-size>` → symbolic track size.
fn track_size(
    t: &lightningcss::properties::grid::TrackSize,
    lctx: &LengthContext,
) -> crate::values::GridTrackSize {
    use crate::values::GridTrackSize as G;
    use lightningcss::properties::grid::TrackSize as TS;
    match t {
        TS::TrackBreadth(tb) => track_breadth(tb, lctx),
        TS::MinMax { min, max } => G::MinMax {
            min: Box::new(track_breadth(min, lctx)),
            max: Box::new(track_breadth(max, lctx)),
        },
        // fit-content(<length-percentage>) ≈ auto growth capped at the
        // argument; v1 maps it to min-content (documented approximation).
        TS::FitContent(_) => G::MinContent,
    }
}

fn track_breadth(
    tb: &lightningcss::properties::grid::TrackBreadth,
    lctx: &LengthContext,
) -> crate::values::GridTrackSize {
    use crate::values::GridTrackSize as G;
    use lightningcss::properties::grid::TrackBreadth as TB;
    match tb {
        TB::Auto => G::Auto,
        TB::MinContent => G::MinContent,
        TB::MaxContent => G::MaxContent,
        TB::Flex(f) => G::Fr(*f),
        TB::Length(lp) => match length_percentage_to_len(lp, lctx) {
            Some(Len::Px(px)) => G::Px(px),
            Some(Len::Percent(p)) => G::Percent(p),
            Some(Len::Calc(c)) => G::Px(c.px_part()),
            None => G::Auto,
        },
    }
}

/// One `<grid-line>` → symbolic line spec (named lines/areas unsupported).
fn grid_line(gl: &lightningcss::properties::grid::GridLine<'static>) -> crate::values::GridLineSpec {
    use crate::values::GridLineSpec as S;
    use lightningcss::properties::grid::GridLine as GL;
    match gl {
        GL::Auto | GL::Area { .. } => S::Auto,
        GL::Line { index, .. } => S::Line(*index as i16),
        GL::Span { index, .. } => S::Span((*index).max(1) as u16),
    }
}

fn map_display(d: &LcDisplay) -> Display {
    use lightningcss::properties::display::{DisplayInside, DisplayKeyword, DisplayOutside};
    match d {
        LcDisplay::Keyword(DisplayKeyword::None) => Display::None,
        // Table keywords parse as bare keywords (not Pair) in lightningcss.
        LcDisplay::Keyword(
            DisplayKeyword::TableRow
            | DisplayKeyword::TableRowGroup
            | DisplayKeyword::TableHeaderGroup
            | DisplayKeyword::TableFooterGroup,
        ) => Display::TableRow,
        LcDisplay::Keyword(DisplayKeyword::TableCell) => Display::TableCell,
        LcDisplay::Keyword(_) => Display::Block,
        LcDisplay::Pair(p) => match &p.inside {
            DisplayInside::Flex(_) | DisplayInside::Box(_) => Display::Flex,
            DisplayInside::Grid => Display::Grid,
            // CSS tables → anonymous flex structures (v1 approximation).
            DisplayInside::Table => Display::Table,
            DisplayInside::Flow if matches!(p.outside, DisplayOutside::Inline) => Display::Inline,
            _ => Display::Block,
        },
    }
}
