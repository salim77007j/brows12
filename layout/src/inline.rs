//! # Inline layout
//!
//! The line-box engine: shapes styled inline runs (text from nested inline
//! elements like `<a>`, `<b>`, `<span>`) and assembles them into wrapped
//! lines — the way real browsers flow inline content across element
//! boundaries inside a block container.
//!
//! Architecture note: shaping happens per inline item with cosmic-text
//! (max-content), line breaking happens here (greedy, word-based), and the
//! resulting positioned segments are handed to the rasterizer. The
//! rasterizer never re-wraps, so measure and paint cannot drift apart.

use crate::TextMeasurer;
use brows12_css::values::{Rgba, TextAlign, WhiteSpace};

/// One styled inline run produced by flattening a group of inline-level
/// DOM nodes (text nodes + inline elements) in tree order.
#[derive(Debug, Clone)]
pub struct InlineItem {
    pub text: String,
    pub font_size: f32,
    pub line_height_px: f32,
    pub font_weight: u16,
    pub italic: bool,
    pub font_family: Option<String>,
    pub color: Rgba,
    pub underline: bool,
    pub line_through: bool,
    pub background: Option<Rgba>,
    pub white_space: WhiteSpace,
}

impl InlineItem {
    pub(crate) fn newline(
        font_size: f32,
        line_height_px: f32,
        style: &brows12_css::ComputedStyle,
    ) -> Self {
        InlineItem {
            text: "\n".into(),
            font_size,
            line_height_px,
            font_weight: style.font_weight,
            italic: false,
            font_family: style.font_family.clone(),
            color: style.color,
            underline: false,
            line_through: false,
            background: None,
            white_space: WhiteSpace::Normal,
        }
    }
}

/// A paint-ready positioned run on a line.
#[derive(Debug, Clone)]
pub struct InlineSegment {
    /// Which `InlineItem` this segment came from (merge key within a line).
    pub item: usize,
    /// Exact substring to paint (never empty, never only spaces).
    pub text: String,
    /// X offset of the segment start within the flow box (alignment applied).
    pub x: f32,
    /// Advance width of the segment.
    pub width: f32,
    pub font_size: f32,
    pub line_height_px: f32,
    pub font_weight: u16,
    pub italic: bool,
    pub font_family: Option<String>,
    pub color: Rgba,
    pub underline: bool,
    pub line_through: bool,
    pub background: Option<Rgba>,
}

/// One assembled line box.
#[derive(Debug, Clone)]
pub struct InlineLine {
    /// Top of the line within the flow box.
    pub y: f32,
    pub height: f32,
    /// Baseline y within the flow box.
    pub baseline: f32,
    pub width: f32,
    pub segments: Vec<InlineSegment>,
}

/// A fully laid-out inline flow (one per inline group).
#[derive(Debug, Clone, Default)]
pub struct InlineFlowLayout {
    pub lines: Vec<InlineLine>,
    /// Width of the widest line (max-content width).
    pub width: f32,
    /// Total height of all lines.
    pub height: f32,
}

const ASCENT_FACTOR: f32 = 0.80;
const DESCENT_FACTOR: f32 = 0.22;

#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    item: usize,
    /// Byte range into `items[item].text`.
    start: usize,
    end: usize,
    width: f32,
    is_space: bool,
    is_break: bool,
    _p: std::marker::PhantomData<&'a ()>,
}

/// Collapse CSS whitespace in a normal-flow text chunk: `\s+` → one space.
/// Leading/trailing single spaces are PRESERVED here (per CSS they collapse
/// to one space; removal happens at line-box boundaries during wrap).
pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    if pending_space {
        out.push(' ');
    }
    out
}

/// Shape one item at max-content and return its glyphs
/// (byte range, x position, advance) plus natural width.
fn shape_item(measurer: &TextMeasurer, item: &InlineItem) -> Vec<(usize, usize, f32, f32)> {
    let mut fs = measurer.font_system.lock().unwrap();
    let metrics = cosmic_text::Metrics::new(item.font_size, item.line_height_px);
    let mut buffer = cosmic_text::Buffer::new(&mut fs, metrics);
    buffer.set_size(None, None);

    let mut attrs = cosmic_text::Attrs::new();
    attrs = match item.font_family.as_deref() {
        Some("monospace") => attrs.family(cosmic_text::Family::Monospace),
        Some("serif") => attrs.family(cosmic_text::Family::Serif),
        Some(name) => attrs.family(cosmic_text::Family::Name(name)),
        None => attrs.family(cosmic_text::Family::SansSerif),
    };
    attrs = attrs.weight(cosmic_text::Weight(item.font_weight));
    if item.italic {
        attrs = attrs.style(cosmic_text::Style::Italic);
    }
    buffer.set_text(&item.text, &attrs, cosmic_text::Shaping::Advanced, None);
    buffer.shape_until_scroll(&mut fs, false);

    let mut glyphs = Vec::new();
    for run in buffer.layout_runs() {
        let n = run.glyphs.len();
        for (i, g) in run.glyphs.iter().enumerate() {
            let advance = if i + 1 < n { run.glyphs[i + 1].x - g.x } else { g.w };
            glyphs.push((g.start, g.end, g.x, advance));
        }
    }
    glyphs
}

/// Shape + break + align an inline flow. `max_width = None` means
/// max-content (no wrapping).
pub fn layout_inline(
    measurer: &TextMeasurer,
    items: &[InlineItem],
    max_width: Option<f32>,
) -> InlineFlowLayout {
    if items.is_empty() {
        return InlineFlowLayout::default();
    }
    let pre_any = items.iter().any(|i| i.white_space == WhiteSpace::Pre);
    let nowrap = items.iter().any(|i| i.white_space == WhiteSpace::NoWrap) && !pre_any;

    // Shape every item once.
    let shaped: Vec<Vec<(usize, usize, f32, f32)>> =
        items.iter().map(|it| shape_item(measurer, it)).collect();

    // Tokenize into words / spaces / hard breaks.
    let mut tokens: Vec<Token> = Vec::new();
    for (idx, (item, gs)) in items.iter().zip(&shaped).enumerate() {
        if item.text == "\n" {
            tokens.push(Token {
                item: idx,
                start: 0,
                end: 1,
                width: 0.0,
                is_space: false,
                is_break: true,
                _p: std::marker::PhantomData,
            });
            continue;
        }
        let bytes = item.text.as_bytes();
        let mut i = 0;
        while i < gs.len() {
            let (s, _e, _x, adv) = gs[i];
            let is_space = bytes.get(s) == Some(&b' ');
            // Merge consecutive glyphs of the same kind into one token.
            // Glyphs come back in VISUAL order (BiDi reordering) — byte
            // ranges may go backwards, so track min/max and use absolute
            // advances.
            let mut j = i;
            let mut min_start = s;
            let mut max_end = _e;
            let mut width = adv.abs();
            while j + 1 < gs.len() {
                let (ns, ne, _nx, nadv) = gs[j + 1];
                let next_space = bytes.get(ns) == Some(&b' ');
                if next_space == is_space {
                    j += 1;
                    width += nadv.abs();
                    min_start = min_start.min(ns);
                    max_end = max_end.max(ne);
                } else {
                    break;
                }
            }
            tokens.push(Token {
                item: idx,
                start: min_start,
                end: max_end,
                width,
                is_space,
                is_break: false,
                _p: std::marker::PhantomData,
            });
            i = j + 1;
        }
    }

    // Break tokens into lines.
    let mut line_tokens: Vec<Vec<Token>> = Vec::new();
    let mut cur: Vec<Token> = Vec::new();
    let mut cur_w = 0.0f32;
    let limit = if pre_any || nowrap { None } else { max_width.filter(|w| *w > 0.0) };
    for tok in tokens {
        if tok.is_break {
            line_tokens.push(std::mem::take(&mut cur));
            cur_w = 0.0;
            continue;
        }
        if tok.is_space && cur.is_empty() {
            continue; // drop leading spaces
        }
        let fits = match limit {
            None => true,
            Some(w) => cur_w + tok.width <= w + 0.5,
        };
        if fits || cur.is_empty() {
            cur.push(tok);
            cur_w += tok.width;
        } else {
            // Break: drop trailing spaces of the finished line.
            while cur.last().map(|t| t.is_space).unwrap_or(false) {
                cur.pop();
            }
            line_tokens.push(std::mem::take(&mut cur));
            cur_w = 0.0;
            if tok.is_space {
                continue; // drop the space at the break
            }
            cur.push(tok);
            cur_w += tok.width;
        }
    }
    if !cur.is_empty() {
        line_tokens.push(cur);
    }

    // Assemble line boxes.
    let mut flow = InlineFlowLayout::default();
    let mut y = 0.0f32;
    for toks in line_tokens {
        if toks.is_empty() {
            continue;
        }
        let width: f32 = toks.iter().map(|t| t.width).sum();
        let mut max_ascent = 0.0f32;
        let mut max_descent = 0.0f32;
        let mut line_h = 0.0f32;
        for t in &toks {
            let it = &items[t.item];
            max_ascent = max_ascent.max(it.font_size * ASCENT_FACTOR);
            max_descent = max_descent.max(it.font_size * DESCENT_FACTOR);
            line_h = line_h.max(it.line_height_px);
        }
        line_h = line_h.max(max_ascent + max_descent);
        let half_leading = ((line_h - max_ascent - max_descent) / 2.0).max(0.0);
        let baseline = y + half_leading + max_ascent;

        // Merge adjacent tokens from the same styled item into segments
        // (fewer paint buffers; style identical by construction), tracking
        // the running x position of each token within the line.
        let mut segments: Vec<InlineSegment> = Vec::new();
        let mut x = 0.0f32;
        for t in &toks {
            let it = &items[t.item];
            if let Some(last) = segments.last_mut() {
                if last.item == t.item {
                    // Same item: extend with the token's byte range (BiDi
                    // may reorder, so append defensively via min/max).
                    if let Some(extra) = it.text.get(t.start..t.end) {
                        last.text.push_str(extra);
                    }
                    last.width += t.width;
                    x += t.width;
                    continue;
                }
            }
            segments.push(InlineSegment {
                item: t.item,
                text: it.text.get(t.start..t.end).unwrap_or("").to_string(),
                x,
                width: t.width,
                font_size: it.font_size,
                line_height_px: it.line_height_px,
                font_weight: it.font_weight,
                italic: it.italic,
                font_family: it.font_family.clone(),
                color: it.color,
                underline: it.underline,
                line_through: it.line_through,
                background: it.background,
            });
            x += t.width;
        }
        flow.lines.push(InlineLine { y, height: line_h, baseline, width, segments });
        y += line_h;
    }
    flow.width = flow.lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
    flow.height = y;
    flow
}

/// Apply text-align: shift each line's segments within `box_width`.
pub fn apply_alignment(flow: &mut InlineFlowLayout, box_width: f32, align: TextAlign) {
    for line in &mut flow.lines {
        let dx = match align {
            TextAlign::Center => ((box_width - line.width) / 2.0).max(0.0),
            TextAlign::Right => (box_width - line.width).max(0.0),
            _ => 0.0,
        };
        if dx > 0.0 {
            for seg in &mut line.segments {
                seg.x += dx;
            }
        }
    }
}
