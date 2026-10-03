//! Stylesheet parsing via lightningcss into fully-owned rule storage.
//!
//! v0.2: at-rule aware. `@media`, `@supports`, `@layer`, `@container`
//! conditions attach to style rules and are evaluated during cascade;
//! `@keyframes` and `@font-face` are collected into the StyleEngine.

use crate::atr::{
    EasingKeyword, KeyframesMap, LayerRegistry, OwnedFontFace, OwnedKeyframe, OwnedKeyframes,
};
use crate::values::Rgba;
use lightningcss::media_query::MediaList as LcMediaList;
use lightningcss::rules::container::{ContainerCondition, ContainerRule};
use lightningcss::rules::font_face::{FontFaceProperty, Source};
use lightningcss::rules::keyframes::{Keyframe as LcKeyframe, KeyframeSelector, KeyframesRule};
use lightningcss::rules::layer::{LayerBlockRule, LayerName};
use lightningcss::rules::media::MediaRule;
use lightningcss::rules::style::StyleRule as LcStyleRule;
use lightningcss::rules::supports::{SupportsCondition as LcSupportsCondition, SupportsRule};
use lightningcss::rules::CssRule;
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::{ParserOptions, StyleSheet as LcStyleSheet};
use lightningcss::traits::IntoOwned;

/// Origin of a stylesheet, used for cascade ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// Built-in browser defaults (lowest priority).
    UserAgent,
    /// Author stylesheets (`<link>` / `<style>` / inline).
    Author,
}

/// One parsed style rule ready for matching, with its at-rule context.
#[derive(Debug, Clone)]
pub struct StyleRule {
    /// Owned selectors (lifetimes erased via `into_owned`).
    pub selectors: SelectorList<'static>,
    /// `!important` declarations.
    pub important: Vec<lightningcss::properties::Property<'static>>,
    /// Normal declarations.
    pub declarations: Vec<lightningcss::properties::Property<'static>>,
    /// Precomputed packed specificity (a<<20 | b<<10 | c).
    pub specificity: u32,
    /// Global source order across all sheets of the document.
    pub order: u32,
    /// Origin for cascade ordering.
    pub origin: Origin,
    /// Every `@media` list enclosing this rule (all must match).
    pub media: Vec<LcMediaList<'static>>,
    /// Enclosing `@supports` conditions (all must match).
    pub supports: Vec<LcSupportsCondition<'static>>,
    /// Enclosing `@container` conditions (all must match).
    pub containers: Vec<ContainerCondition<'static>>,
    /// Cascade layer path (`None` = unlayered).
    pub layer: Option<String>,
}

/// A parsed stylesheet with its collected named at-rules.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<StyleRule>,
    /// `@keyframes` by animation name.
    pub keyframes: KeyframesMap,
    /// `@font-face` rules in source order.
    pub font_faces: Vec<OwnedFontFace>,
    /// Cascade layer declaration order.
    pub layers: LayerRegistry,
    /// Custom properties declared anywhere in this sheet (document-global
    /// scope model; later declarations win).
    pub custom_defs: std::collections::HashMap<String, String>,
}

/// Everything collected from a set of stylesheets.
#[derive(Debug, Clone, Default)]
pub struct StyleEngine {
    pub rules: Vec<StyleRule>,
    /// `@keyframes` by name.
    pub keyframes: KeyframesMap,
    /// `@font-face` rules in source order.
    pub font_faces: Vec<OwnedFontFace>,
    /// Cascade layer declaration order.
    pub layers: LayerRegistry,
    /// Document-global custom property values (merged from all sheets).
    pub global_vars: std::collections::HashMap<String, String>,
}

impl StyleRule {
    fn specificity_of(selectors: &SelectorList<'static>) -> u32 {
        selectors.0.iter().map(|s| s.specificity()).max().unwrap_or(0)
    }
}

impl StyleEngine {
    /// Build an engine from ordered stylesheets (UA sheet first).
    pub fn new(sheets: &[Stylesheet]) -> Self {
        let combined = Stylesheet::combine(sheets);
        StyleEngine {
            rules: combined.rules,
            keyframes: combined.keyframes,
            font_faces: combined.font_faces,
            layers: combined.layers,
            global_vars: combined.custom_defs,
        }
    }

    /// Read-only view of the merged rules.
    pub fn rules(&self) -> &[StyleRule] {
        &self.rules
    }
}

/// Traversal context carried down the rule tree.
#[derive(Default, Clone)]
struct AtCtx {
    media: Vec<LcMediaList<'static>>,
    supports: Vec<LcSupportsCondition<'static>>,
    containers: Vec<ContainerCondition<'static>>,
    layer: Option<String>,
}

/// Rewrite `float:` / `clear:` declarations into reserved custom
/// properties (`--brows-float` / `--brows-clear`) so lightningcss keeps
/// them. Only declaration-name positions are rewritten: the name must be
/// preceded by `{`, `;`, or the start of the text (optionally with
/// whitespace), which selectors (`a.float:hover`), values
/// (`animation-name: float`) and function arguments never match.
pub(crate) fn rewrite_float_decls(css: &str) -> String {
    let lower_has = css.to_ascii_lowercase();
    if !lower_has.contains("float") && !lower_has.contains("clear") {
        return css.to_string();
    }
    let bytes = css.as_bytes();
    let mut out = String::with_capacity(css.len() + 16);
    let mut i = 0usize;
    // At a declaration position? (start / after '{' / after ';')
    let mut decl_pos = true;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'{' || b == b';' {
            decl_pos = true;
            out.push(b as char);
            i += 1;
            continue;
        }
        if b == b'}' {
            decl_pos = false;
            out.push(b as char);
            i += 1;
            continue;
        }
        if b.is_ascii_whitespace() {
            out.push(b as char);
            i += 1;
            continue;
        }
        if !decl_pos {
            // Copy a run up to the next structural boundary verbatim.
            let start = i;
            while i < bytes.len() && bytes[i] != b'{' && bytes[i] != b';' {
                i += 1;
            }
            out.push_str(&css[start..i]);
            decl_pos = false;
            continue;
        }
        // Declaration name position: try to match `float:` / `clear:`.
        let rest = &css[i..];
        let lower = rest.to_ascii_lowercase();
        let (keyword, replacement) = if lower.starts_with("float") {
            ("float", "--brows-float")
        } else if lower.starts_with("clear") {
            ("clear", "--brows-clear")
        } else {
            ("", "")
        };
        if !keyword.is_empty() {
            let after = &lower[keyword.len()..];
            let ws = after.len() - after.trim_start().len();
            // `after` starts right after the keyword; the colon (if any)
            // follows the optional whitespace.
            if after[ws..].starts_with(':') {
                out.push_str(replacement);
                // Copy `keyword` + whitespace + ':' verbatim from the input.
                out.push_str(&rest[keyword.len()..keyword.len() + ws + 1]);
                i += keyword.len() + ws + 1;
                decl_pos = false;
                continue;
            }
        }
        // Not float/clear: copy the name up to ':' or structural char.
        let start = i;
        while i < bytes.len() && bytes[i] != b':' && bytes[i] != b'{' && bytes[i] != b';' {
            i += 1;
        }
        out.push_str(&css[start..i]);
        if i < bytes.len() && bytes[i] == b':' {
            out.push(':');
            i += 1;
            decl_pos = false;
        }
    }
    out
}

impl Stylesheet {
    /// Parse CSS source into a stylesheet. Rules keep their source order.
    /// Custom properties are resolved at the text level before parsing
    /// (see `vartext`): this makes `var()` design tokens work for the
    /// document-global scope model v0.2 ships.
    pub fn parse(css: &str, origin: Origin) -> Result<Self, crate::CssError> {
        Self::parse_with_env(css, origin, crate::vartext::ScopeEnv::default())
    }

    /// Like [`parse`], with the device facts the scoped design-token
    /// collector needs (`prefers-color-scheme`, viewport queries).
    pub fn parse_with_env(
        css: &str,
        origin: Origin,
        env: crate::vartext::ScopeEnv,
    ) -> Result<Self, crate::CssError> {
        // lightningcss does not model `float`/`clear`; rewrite them into
        // reserved custom properties (declaration-position only) so they
        // survive parsing and reach the cascade.
        let css = rewrite_float_decls(css);

        // Harvest custom property definitions from the raw text — only
        // where the document-global scope model is sound (root-scoped
        // selectors under at-rule conditions that hold for this device).
        // A theme-qualified rule like `html.skin-x { --bg: #111 }` must
        // not repaint pages whose root never matches it.
        let mut custom_defs = std::collections::HashMap::new();
        crate::vartext::collect_root_scoped_defs(&css, &mut custom_defs, &env);

        // If the sheet references vars, substitute and re-parse.
        let effective = if css.contains("var(") {
            crate::vartext::substitute_vars_text(&css, &custom_defs)
        } else {
            css
        };

        // error_recovery: browsers skip declarations/rules they do not
        // understand — one exotic at-rule must not discard the whole sheet
        // (real-world pages mix syntax generations).
        let sheet = LcStyleSheet::parse(
            &effective,
            ParserOptions { error_recovery: true, ..ParserOptions::default() },
        )
        .map_err(|e| crate::CssError::Parse(format!("{e}")))?;
        // Erase lifetimes: the owned stylesheet no longer borrows `css`.
        let sheet: LcStyleSheet<'static> = IntoOwned::into_owned(sheet);
        let mut rules = Vec::new();
        let mut registry = Registry::default();
        collect_rules(&sheet.rules, origin, &AtCtx::default(), &mut rules, &mut 0, &mut registry);
        Ok(Stylesheet {
            rules,
            keyframes: registry.keyframes,
            font_faces: registry.font_faces,
            layers: registry.layers,
            custom_defs,
        })
    }

    /// Combine several stylesheets (their order defines source order).
    pub fn combine(sheets: &[Stylesheet]) -> Stylesheet {
        let mut rules = Vec::new();
        let mut order = 0u32;
        let mut keyframes = KeyframesMap::default();
        let mut font_faces = Vec::new();
        let mut layers = LayerRegistry::default();
        let mut custom_defs = std::collections::HashMap::new();
        for sheet in sheets {
            for rule in &sheet.rules {
                let mut r = rule.clone();
                r.order = order;
                order += 1;
                rules.push(r);
            }
            keyframes.extend(sheet.keyframes.clone());
            font_faces.extend(sheet.font_faces.iter().cloned());
            for l in &sheet.layers.order {
                layers.declare(l);
            }
            for (k, v) in &sheet.custom_defs {
                custom_defs.insert(k.clone(), v.clone());
            }
        }
        Stylesheet { rules, keyframes, font_faces, layers, custom_defs }
    }
}

/// Collects named at-rule entities during the walk.
#[derive(Default)]
struct Registry {
    keyframes: KeyframesMap,
    font_faces: Vec<OwnedFontFace>,
    layers: LayerRegistry,
}

fn layer_path_string(name: &LayerName) -> String {
    name.0.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" ")
}

fn collect_rules(
    list: &lightningcss::rules::CssRuleList<'static>,
    origin: Origin,
    ctx: &AtCtx,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
    registry: &mut Registry,
) {
    for rule in &list.0 {
        match rule {
            CssRule::Style(style_rule) => {
                push_style_rule(style_rule, origin, ctx, out, counter);
                // Nested rules (CSS Nesting) inherit the at-rule context.
                collect_nested(&style_rule.rules, origin, ctx, out, counter, registry);
            }
            CssRule::Media(media) => collect_media(media, origin, ctx, out, counter, registry),
            CssRule::Supports(supports) => {
                collect_supports(supports, origin, ctx, out, counter, registry)
            }
            CssRule::LayerBlock(layer) => {
                collect_layer_block(layer, origin, ctx, out, counter, registry)
            }
            CssRule::LayerStatement(stmt) => {
                for name in &stmt.names {
                    registry.layers.declare(&layer_path_string(name));
                }
            }
            CssRule::Container(container) => {
                collect_container(container, origin, ctx, out, counter, registry)
            }
            CssRule::Keyframes(kf) => collect_keyframes(kf, registry),
            CssRule::FontFace(ff) => collect_font_face(ff, registry),
            _ => {
                // Other at-rules (charset, import, namespace, page, ...) are
                // intentionally ignored; see docs/CAPABILITY_REPORT.md.
            }
        }
    }
}

fn collect_nested(
    list: &lightningcss::rules::CssRuleList<'static>,
    origin: Origin,
    ctx: &AtCtx,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
    registry: &mut Registry,
) {
    collect_rules(list, origin, ctx, out, counter, registry);
}

fn collect_media(
    media: &MediaRule<'static>,
    origin: Origin,
    ctx: &AtCtx,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
    registry: &mut Registry,
) {
    let mut inner = ctx.clone();
    let list: LcMediaList<'static> = media.query.clone();
    inner.media.push(list);
    collect_rules(&media.rules, origin, &inner, out, counter, registry);
}

fn collect_supports(
    supports: &SupportsRule<'static>,
    origin: Origin,
    ctx: &AtCtx,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
    registry: &mut Registry,
) {
    let mut inner = ctx.clone();
    inner.supports.push(supports.condition.clone());
    collect_rules(&supports.rules, origin, &inner, out, counter, registry);
}

fn collect_layer_block(
    layer: &LayerBlockRule<'static>,
    origin: Origin,
    ctx: &AtCtx,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
    registry: &mut Registry,
) {
    let mut inner = ctx.clone();
    let path = match (&ctx.layer, &layer.name) {
        (Some(parent), Some(name)) => {
            format!("{parent} {}", layer_path_string(name))
        }
        (None, Some(name)) => layer_path_string(name),
        (_, None) => {
            // Anonymous layer: stable synthetic path per declaration site.
            match &ctx.layer {
                Some(parent) => format!("{parent} ~anon{}", *counter),
                None => format!("~anon{}", *counter),
            }
        }
    };
    registry.layers.declare(&path);
    inner.layer = Some(path);
    collect_rules(&layer.rules, origin, &inner, out, counter, registry);
}

fn collect_container(
    container: &ContainerRule<'static>,
    origin: Origin,
    ctx: &AtCtx,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
    registry: &mut Registry,
) {
    let Some(condition) = &container.condition else { return };
    let mut inner = ctx.clone();
    inner.containers.push(condition.clone());
    collect_rules(&container.rules, origin, &inner, out, counter, registry);
}

fn collect_keyframes(kf: &KeyframesRule<'static>, registry: &mut Registry) {
    let name = crate::atr::to_css_string(&kf.name).unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let mut frames = Vec::new();
    for k in &kf.keyframes {
        if let Some(frame) = owned_keyframe(k) {
            frames.push(frame);
        }
    }
    frames.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal));
    registry.keyframes.insert(name.clone(), OwnedKeyframes { name, frames });
}

fn owned_keyframe(k: &LcKeyframe<'static>) -> Option<OwnedKeyframe> {
    let mut offset: Option<f32> = None;
    for sel in &k.selectors {
        let o = match sel {
            KeyframeSelector::Percentage(p) => p.0,
            KeyframeSelector::From => 0.0,
            KeyframeSelector::To => 1.0,
            KeyframeSelector::TimelineRangePercentage(_) => continue,
        };
        offset = Some(offset.map_or(o, |cur: f32| cur.max(o)));
    }
    let mut declarations = Vec::new();
    let mut important = Vec::new();
    let mut easing = None;
    for prop in &k.declarations.declarations {
        extract_easing(prop, &mut easing);
        declarations.push(prop.clone());
    }
    for prop in &k.declarations.important_declarations {
        extract_easing(prop, &mut easing);
        important.push(prop.clone());
    }
    Some(OwnedKeyframe { offset: offset?, declarations, important, easing })
}

fn extract_easing(
    prop: &lightningcss::properties::Property<'static>,
    easing: &mut Option<EasingKeyword>,
) {
    if let lightningcss::properties::Property::AnimationTimingFunction(fns, _) = prop {
        if let Some(first) = fns.first() {
            *easing = crate::atr::timing_to_easing(first);
        }
    }
}

fn collect_font_face(
    ff: &lightningcss::rules::font_face::FontFaceRule<'static>,
    registry: &mut Registry,
) {
    let mut family = String::new();
    let mut urls = Vec::new();
    let mut weight = None;
    let mut style_italic = false;
    for prop in &ff.properties {
        match prop {
            FontFaceProperty::FontFamily(f) => {
                family = crate::atr::to_css_string(f)
                    .map(|s| s.trim_matches('"').trim_matches('\'').to_string())
                    .unwrap_or_default();
            }
            FontFaceProperty::Source(srcs) => {
                for s in srcs {
                    if let Source::Url(u) = s {
                        urls.push(u.url.url.to_string());
                    }
                }
            }
            FontFaceProperty::FontWeight(w) => {
                if let lightningcss::properties::font::FontWeight::Absolute(
                    lightningcss::properties::font::AbsoluteFontWeight::Weight(n),
                ) = w.0
                {
                    weight = Some(n as u16);
                }
            }
            FontFaceProperty::FontStyle(s) => {
                if matches!(s, lightningcss::rules::font_face::FontStyle::Italic) {
                    style_italic = true;
                }
            }
            _ => {}
        }
    }
    if !family.is_empty() && !urls.is_empty() {
        registry.font_faces.push(OwnedFontFace { family, urls, weight, style_italic });
    }
}

fn push_style_rule(
    rule: &LcStyleRule<'static>,
    origin: Origin,
    ctx: &AtCtx,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
) {
    let specificity = StyleRule::specificity_of(&rule.selectors);
    out.push(StyleRule {
        selectors: rule.selectors.clone(),
        important: rule.declarations.important_declarations.clone(),
        declarations: rule.declarations.declarations.clone(),
        specificity,
        order: *counter,
        origin,
        media: ctx.media.clone(),
        supports: ctx.supports.clone(),
        containers: ctx.containers.clone(),
        layer: ctx.layer.clone(),
    });
    *counter += 1;
}

/// Reference to a single selector for matching APIs.
pub type SelectorRef<'a> = &'a lightningcss::selector::Selector<'static>;

/// Packed specificity helper (for tests and debugging).
pub fn specificity_tuple(packed: u32) -> (u32, u32, u32) {
    (packed >> 20 & 0x3ff, packed >> 10 & 0x3ff, packed & 0x3ff)
}

/// Convert a lightningcss color into our RGBA, resolving `currentColor`.
pub fn resolve_color(
    color: &lightningcss::values::color::CssColor,
    inherited: Rgba,
) -> Option<Rgba> {
    use lightningcss::values::color::CssColor;
    match color {
        CssColor::CurrentColor => Some(inherited),
        CssColor::RGBA(rgba) => Some([rgba.red, rgba.green, rgba.blue, rgba.alpha]),
        other => {
            let rgba = lightningcss::values::color::RGBA::try_from(other.clone()).ok()?;
            Some([rgba.red, rgba.green, rgba.blue, rgba.alpha])
        }
    }
}

/// A very small helper used by the UA stylesheet builder.
pub const UA_RESET: &str = include_str!("ua.css");

#[cfg(test)]
mod rewrite_tests {
    use super::*;

    #[test]
    fn rewrite_basic() {
        let out = rewrite_float_decls(".infobox { float: right; }");
        println!("OUT1: {out}");
        assert!(out.contains("--brows-float: right"), "got: {out}");
        let out2 = rewrite_float_decls("a { clear: both; float:left; color: red }");
        println!("OUT2: {out2}");
        assert!(out2.contains("--brows-clear: both"));
        assert!(out2.contains("--brows-float:left"));
        assert!(out2.contains("color: red"));
        let out3 = rewrite_float_decls("a.float:hover { color: blue }");
        println!("OUT3: {out3}");
        assert!(!out3.contains("--brows-float"), "selector must not be rewritten");
    }

    #[test]
    fn custom_prop_survives_parse() {
        let sheet = Stylesheet::parse(".infobox { --brows-float: right; }", Origin::Author).unwrap();
        let mut found = false;
        for r in &sheet.rules {
            for p in &r.declarations {
                println!("DECL: {p:?}");
                if matches!(p, lightningcss::properties::Property::Custom(_)) {
                    found = true;
                }
            }
        }
        assert!(found, "custom property must survive lightningcss parse");
    }
}
