//! Application of lightningcss `Property` values to [`ComputedStyle`].

use crate::computed::*;
use crate::values::*;
use lightningcss::properties::Property;
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::display::Display as LcDisplay;
use lightningcss::properties::font::{
    AbsoluteFontSize, AbsoluteFontWeight, FontSize, FontWeight, RelativeFontSize,
};
use lightningcss::properties::position::{Position as LcPosition, ZIndex as LcZIndex};
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::text::{TextAlign as LcTextAlign, WhiteSpace as LcWhiteSpace};
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
        Property::BackgroundColor(c) => {
            if let Some(rgba) = color(c, s.color) {
                s.background_color = rgba;
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
            let resolved = match fs {
                FontSize::Length(lp) => crate::computed::length_percentage_to_len(lp, &lctx)
                    .map(|len| match len {
                        Len::Px(px) => px,
                        Len::Percent(p) => parent_size * p / 100.0,
                    }),
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
                LcLH::Length(lp) => {
                    crate::computed::length_percentage_to_len(lp, &lctx)
                        .map(|len| match len {
                            Len::Px(px) => LineHeight::Px(px),
                            Len::Percent(p) => LineHeight::Px(s.font_size * p / 100.0),
                        })
                        .unwrap_or(LineHeight::Normal)
                }
            };
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
        Property::BorderTopStyle(v) => s.border_width.top = style_width(s.border_width.top, v),
        Property::BorderRightStyle(v) => s.border_width.right = style_width(s.border_width.right, v),
        Property::BorderBottomStyle(v) => {
            s.border_width.bottom = style_width(s.border_width.bottom, v)
        }
        Property::BorderLeftStyle(v) => s.border_width.left = style_width(s.border_width.left, v),
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
            let r = (avg(&br.top_left) + avg(&br.top_right) + avg(&br.bottom_right) + avg(&br.bottom_left)) / 4.0;
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
            s.text_underline = td.line.contains(lightningcss::properties::text::TextDecorationLine::Underline);
            s.text_line_through = td.line.contains(lightningcss::properties::text::TextDecorationLine::LineThrough);
        }
        Property::TextDecorationLine(line, _) => {
            s.text_underline = line.contains(lightningcss::properties::text::TextDecorationLine::Underline);
            s.text_line_through = line.contains(lightningcss::properties::text::TextDecorationLine::LineThrough);
        }
        Property::FlexDirection(d, _) => {
            s.flex_direction = match d {
                lightningcss::properties::flex::FlexDirection::Row => FlexDirection::Row,
                lightningcss::properties::flex::FlexDirection::RowReverse => FlexDirection::RowReverse,
                lightningcss::properties::flex::FlexDirection::Column => FlexDirection::Column,
                lightningcss::properties::flex::FlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
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
            use lightningcss::properties::align::{ContentDistribution, ContentPosition, JustifyContent as LcJ};
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
        Property::Overflow(o) => s.overflow = map_overflow(o.y),
        Property::OverflowY(o) => s.overflow = map_overflow(*o),
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
        _ => {
            // Unrecognized property: intentionally ignored in v1.
        }
    }
}

fn color(c: &CssColor, inherited: Rgba) -> Option<Rgba> {
    crate::stylesheet::resolve_color(c, inherited)
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

fn max_size(v: &MaxSize, lctx: &LengthContext) -> Option<AutoPx> {
    match v {
        MaxSize::None => Some(AutoPx::Auto),
        MaxSize::LengthPercentage(lp) => {
            crate::computed::length_percentage_to_len(lp, lctx).map(AutoPx::Len)
        }
        _ => None,
    }
}

fn side_width(v: &BorderSideWidth, _lctx: &LengthContext) -> Len {
    match v {
        BorderSideWidth::Thin => Len::Px(1.0),
        BorderSideWidth::Medium => Len::Px(3.0),
        BorderSideWidth::Thick => Len::Px(5.0),
        BorderSideWidth::Length(l) => l.to_px().map(Len::Px).unwrap_or(Len::Px(0.0)),
    }
}

fn style_width(current: Len, style: &LineStyle) -> Len {
    match style {
        LineStyle::None | LineStyle::Hidden => Len::Px(0.0),
        _ => {
            if matches!(current, Len::Px(0.0)) {
                Len::Px(3.0)
            } else {
                current
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
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

fn map_display(d: &LcDisplay) -> Display {
    use lightningcss::properties::display::{DisplayInside, DisplayKeyword, DisplayOutside};
    match d {
        LcDisplay::Keyword(DisplayKeyword::None) => Display::None,
        LcDisplay::Keyword(_) => Display::Block,
        LcDisplay::Pair(p) => match &p.inside {
            DisplayInside::Flex(_) | DisplayInside::Box(_) => Display::Flex,
            DisplayInside::Flow if matches!(p.outside, DisplayOutside::Inline) => Display::Inline,
            _ => Display::Block,
        },
    }
}
