//! Stylesheet parsing via lightningcss into fully-owned rule storage.

use crate::values::Rgba;
use lightningcss::properties::Property;
use lightningcss::rules::style::StyleRule as LcStyleRule;
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

/// One parsed style rule ready for matching.
#[derive(Debug, Clone)]
pub struct StyleRule {
    /// Owned selectors (lifetimes erased via `into_owned`).
    pub selectors: SelectorList<'static>,
    /// `!important` declarations.
    pub important: Vec<Property<'static>>,
    /// Normal declarations.
    pub declarations: Vec<Property<'static>>,
    /// Precomputed packed specificity (a<<20 | b<<10 | c).
    pub specificity: u32,
    /// Global source order across all sheets of the document.
    pub order: u32,
    /// Origin for cascade ordering.
    pub origin: Origin,
}

/// A parsed stylesheet.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<StyleRule>,
}

/// The rule set used for one document: UA + author styles merged and ready
/// for matching.
#[derive(Debug, Clone, Default)]
pub struct StyleEngine {
    pub rules: Vec<StyleRule>,
}

impl StyleEngine {
    /// Build an engine from ordered stylesheets (UA sheet first).
    pub fn new(sheets: &[Stylesheet]) -> Self {
        StyleEngine { rules: Stylesheet::combine(sheets).rules }
    }

    /// Read-only view of the merged rules.
    pub fn rules(&self) -> &[StyleRule] {
        &self.rules
    }
}

impl Stylesheet {
    /// Parse CSS source into a stylesheet. Rules keep their source order.
    pub fn parse(css: &str, origin: Origin) -> Result<Self, crate::CssError> {
        let sheet = LcStyleSheet::parse(css, ParserOptions::default())
            .map_err(|e| crate::CssError::Parse(format!("{e}")))?;
        // Erase lifetimes: the owned stylesheet no longer borrows `css`.
        let sheet: LcStyleSheet<'static> = IntoOwned::into_owned(sheet);
        let mut rules = Vec::new();
        collect_rules(&sheet, origin, &mut rules, &mut 0);
        Ok(Stylesheet { rules })
    }

    /// Combine several stylesheets (their order defines source order).
    pub fn combine(sheets: &[Stylesheet]) -> Stylesheet {
        let mut rules = Vec::new();
        let mut order = 0u32;
        for sheet in sheets {
            for rule in &sheet.rules {
                let mut r = rule.clone();
                r.order = order;
                order += 1;
                rules.push(r);
            }
        }
        Stylesheet { rules }
    }
}

fn collect_rules(
    sheet: &LcStyleSheet<'static>,
    origin: Origin,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
) {
    for rule in &sheet.rules.0 {
        if let CssRule::Style(style_rule) = rule {
            push_style_rule(style_rule, origin, out, counter);
            // Nested rules (CSS Nesting) are collected with their own
            // specificity; `&` components resolve to false for now.
            collect_nested(&style_rule.rules, origin, out, counter);
        }
        // v1 ignores at-rules other than inline style rules nested in
        // them; see docs/ROADMAP.md (media queries, @supports).
    }
}

fn collect_nested(
    list: &lightningcss::rules::CssRuleList<'static>,
    origin: Origin,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
) {
    for rule in &list.0 {
        if let CssRule::Style(style_rule) = rule {
            push_style_rule(style_rule, origin, out, counter);
            collect_nested(&style_rule.rules, origin, out, counter);
        }
    }
}

fn push_style_rule(
    rule: &LcStyleRule<'static>,
    origin: Origin,
    out: &mut Vec<StyleRule>,
    counter: &mut u32,
) {
    let specificity = rule.selectors.0.iter().map(|s| s.specificity()).max().unwrap_or(0);
    out.push(StyleRule {
        selectors: rule.selectors.clone(),
        important: rule.declarations.important_declarations.clone(),
        declarations: rule.declarations.declarations.clone(),
        specificity,
        order: *counter,
        origin,
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
