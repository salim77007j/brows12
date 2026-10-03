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
    /// Own computed `visibility` of the source node — hidden segments keep
    /// their layout metrics but do not paint (children override per node).
    pub visible: bool,
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
            visible: style.visibility == brows12_css::values::Visibility::Visible,
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
    /// Visibility of the source node (keep metrics, skip paint when false).
    pub visible: bool,
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
    /// Float insets for this line: content starts at `left_inset` and the
    /// line's right limit is `box_width - right_inset`.
    pub left_inset: f32,
    pub right_inset: f32,
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

/// One horizontal obstacle band for float-aware line wrapping, in the
/// flow box's LOCAL coordinates (y measured from the flow top, left/right
/// measured from the flow's left edge).
///
/// Lines overlapping [y0, y1) must start after `left` and end before
/// `width - right` (the caller passes the flow width separately).
#[derive(Debug, Clone, Copy, Default)]
pub struct FloatBand {
    pub y0: f32,
    pub y1: f32,
    /// How far the line's left edge is pushed right by left floats.
    pub left: f32,
    /// How far the line's right edge is pulled in by right floats.
    pub right: f32,
}

/// Left/right insets at local `y` from all bands overlapping the line's
/// height (CSS 2.1 §9.5: a line box shortens around every float whose
/// top edge is above the line's top and whose bottom edge is below it;
/// we use the line's top edge as the reference, the standard simple
/// approximation).
fn insets_at(bands: &[FloatBand], y: f32) -> (f32, f32) {
    let mut left = 0.0f32;
    let mut right = 0.0f32;
    for b in bands {
        if b.y0 <= y + 0.5 && b.y1 > y + 0.5 {
            left = left.max(b.left);
            right = right.max(b.right);
        }
    }
    (left, right)
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
    layout_inline_banded(measurer, items, max_width, &[])
}

/// Float-aware variant: every line box is shortened by the float bands
/// overlapping its vertical position, and line content is offset by the
/// left inset (text wraps around floats exactly like the block engines
/// of major browsers).
pub fn layout_inline_banded(
    measurer: &TextMeasurer,
    items: &[InlineItem],
    max_width: Option<f32>,
    bands: &[FloatBand],
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

    // Break tokens into lines. Band-aware: each line's available width is
    // the box width minus the float insets at the line's start y, and line
    // content starts at the left inset (text flows around floats).
    struct BrokenLine<'a> {
        toks: Vec<Token<'a>>,
        left: f32,
        right: f32,
    }
    let mut lines_out: Vec<BrokenLine> = Vec::new();
    let mut cur: Vec<Token> = Vec::new();
    let mut cur_w = 0.0f32;
    // A zero limit is valid (MinContent intrinsic sizing): wrap at every
    // opportunity so the widest token defines the min-content width.
    let limit = if pre_any || nowrap { None } else { max_width.filter(|w| *w >= 0.0) };
    let mut completed_y = 0.0f32;
    let (mut line_left, mut line_right) = insets_at(bands, completed_y);
    // Height of one line by the same formula the assembly phase uses.
    fn line_h_of(toks: &[Token], items: &[InlineItem]) -> f32 {
        let mut max_ascent = 0.0f32;
        let mut max_descent = 0.0f32;
        let mut lh = 0.0f32;
        for t in toks {
            let it = &items[t.item];
            max_ascent = max_ascent.max(it.font_size * ASCENT_FACTOR);
            max_descent = max_descent.max(it.font_size * DESCENT_FACTOR);
            lh = lh.max(it.line_height_px);
        }
        lh.max(max_ascent + max_descent)
    }
    // Flush the current line at a break point: drop trailing spaces,
    // record the line with its insets, advance the y cursor, re-query the
    // float insets for the next line.
    #[allow(clippy::too_many_arguments)]
    fn break_line<'a>(
        cur: &mut Vec<Token<'a>>,
        items: &[InlineItem],
        bands: &[FloatBand],
        lines_out: &mut Vec<BrokenLine<'a>>,
        cur_w: &mut f32,
        completed_y: &mut f32,
        line_left: &mut f32,
        line_right: &mut f32,
    ) {
        while cur.last().map(|t| t.is_space).unwrap_or(false) {
            cur.pop();
        }
        let h = line_h_of(cur, items);
        lines_out.push(BrokenLine {
            toks: std::mem::take(cur),
            left: *line_left,
            right: *line_right,
        });
        *completed_y += h;
        *cur_w = 0.0;
        let ins = insets_at(bands, *completed_y);
        *line_left = ins.0;
        *line_right = ins.1;
    }
    for tok in tokens {
        if tok.is_break {
            // Hard break (<br>): flush the current line even if empty.
            break_line(
                &mut cur,
                items,
                bands,
                &mut lines_out,
                &mut cur_w,
                &mut completed_y,
                &mut line_left,
                &mut line_right,
            );
            continue;
        }
        if tok.is_space && cur.is_empty() {
            continue; // drop leading spaces
        }
        let avail = match limit {
            None => f32::INFINITY,
            Some(w) => (w - line_left - line_right).max(0.0),
        };
        let fits = cur_w + tok.width <= avail + 0.5;
        if fits || cur.is_empty() {
            cur.push(tok);
            cur_w += tok.width;
        } else {
            // Break: drop trailing spaces of the finished line.
            break_line(
                &mut cur,
                items,
                bands,
                &mut lines_out,
                &mut cur_w,
                &mut completed_y,
                &mut line_left,
                &mut line_right,
            );
            if tok.is_space {
                continue; // drop the space at the break
            }
            cur.push(tok);
            cur_w += tok.width;
        }
    }
    if !cur.is_empty() {
        lines_out.push(BrokenLine { toks: cur, left: line_left, right: line_right });
    }

    // Assemble line boxes.
    let mut flow = InlineFlowLayout::default();
    let mut y = 0.0f32;
    for bl in lines_out {
        let toks = &bl.toks;
        if toks.is_empty() {
            continue;
        }
        let width: f32 = toks.iter().map(|t| t.width).sum();
        let mut max_ascent = 0.0f32;
        let mut max_descent = 0.0f32;
        let mut line_h = 0.0f32;
        for t in toks {
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
        let mut x = bl.left;
        for t in toks {
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
                visible: it.visible,
            });
            x += t.width;
        }
        flow.lines.push(InlineLine {
            y,
            height: line_h,
            baseline,
            width,
            left_inset: bl.left,
            right_inset: bl.right,
            segments,
        });
        y += line_h;
    }
    flow.width = flow.lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
    flow.height = y;
    flow
}

/// Apply text-align: shift each line's segments within `box_width`,
/// honouring per-line float insets (lines align inside their shortened
/// band, like Chrome).
pub fn apply_alignment(flow: &mut InlineFlowLayout, box_width: f32, align: TextAlign) {
    for line in &mut flow.lines {
        let avail = (box_width - line.left_inset - line.right_inset).max(0.0);
        let dx = match align {
            TextAlign::Center => line.left_inset + ((avail - line.width) / 2.0).max(0.0),
            TextAlign::Right => line.left_inset + (avail - line.width).max(0.0),
            _ => line.left_inset,
        };
        let dx = dx - line.left_inset; // segments already start at left_inset
        if dx > 0.0 {
            for seg in &mut line.segments {
                seg.x += dx;
            }
        }
    }
}
