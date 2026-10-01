//! Selector matching against the Brows12 arena DOM.
//!
//! Operates directly on `parcel_selectors` `Component` ASTs (as produced by
//! lightningcss) in *match order* (innermost compound first, combinators
//! separating later, more ancestral compounds).

use brows12_html::{Document, NodeId};
use lightningcss::selector::{Combinator, Component, PseudoClass};
use parcel_selectors::parser::{NthSelectorData, NthType};

/// Does any selector of `list` match element `node`?
pub fn matches_selector_list(
    doc: &Document,
    node: NodeId,
    list: &lightningcss::selector::SelectorList<'static>,
) -> bool {
    list.0.iter().any(|sel| matches_selector(doc, node, sel))
}

/// Does `selector` match element `node`?
pub fn matches_selector<'i>(
    doc: &Document,
    node: NodeId,
    selector: &lightningcss::selector::Selector<'i>,
) -> bool {
    // Rules with pseudo-elements (::before etc.) are not applied in v1.
    if selector.has_pseudo_element() {
        return false;
    }
    let comps: Vec<&Component> = selector.iter_raw_match_order().collect();
    match_sequence(doc, node, &comps)
}

/// Match a (compound, combinator, compound, ...) sequence against `node`.
fn match_sequence<'i>(doc: &Document, node: NodeId, comps: &[&Component<'i>]) -> bool {
    let mut i = 0;
    while i < comps.len() {
        if matches!(comps[i], Component::Combinator(_)) {
            break;
        }
        if !component_matches(doc, node, comps[i]) {
            return false;
        }
        i += 1;
    }
    if i == comps.len() {
        return true;
    }
    match comps[i] {
        Component::Combinator(Combinator::Child) => doc
            .parent(node)
            .filter(|&p| doc.is_element(p))
            .is_some_and(|p| match_sequence(doc, p, &comps[i + 1..])),
        Component::Combinator(Combinator::Descendant) => {
            let mut ancestor = doc.parent(node).filter(|&p| doc.is_element(p));
            while let Some(a) = ancestor {
                if match_sequence(doc, a, &comps[i + 1..]) {
                    return true;
                }
                ancestor = doc.parent(a).filter(|&p| doc.is_element(p));
            }
            false
        }
        Component::Combinator(Combinator::NextSibling) => doc
            .previous_sibling_element(node)
            .is_some_and(|s| match_sequence(doc, s, &comps[i + 1..])),
        Component::Combinator(Combinator::LaterSibling) => {
            let mut prev = doc.previous_sibling_element(node);
            while let Some(s) = prev {
                if match_sequence(doc, s, &comps[i + 1..]) {
                    return true;
                }
                prev = doc.previous_sibling_element(s);
            }
            false
        }
        // Pseudo-element combinators / shadow-DOM parts: not supported in v1.
        _ => false,
    }
}

fn component_matches<'i>(doc: &Document, node: NodeId, comp: &Component<'i>) -> bool {
    match comp {
        Component::ExplicitUniversalType | Component::ExplicitAnyNamespace => true,
        Component::ExplicitNoNamespace
        | Component::DefaultNamespace(_)
        | Component::Namespace(..) => true,
        Component::LocalName(ln) => {
            let tag = doc.local_name(node);
            if doc_is_html(doc, node) {
                &*ln.lower_name.0 == tag || &*ln.name.0 == tag
            } else {
                &*ln.name.0 == tag
            }
        }
        Component::ID(id) => doc.attr(node, "id").is_some_and(|v| v == id.0.as_ref()),
        Component::Class(class) => doc.classes(node).any(|c| c == class.0.as_ref()),
        Component::AttributeInNoNamespaceExists { local_name, .. } => {
            doc.attr(node, local_name.0.as_ref()).is_some()
        }
        Component::AttributeInNoNamespace {
            local_name,
            operator,
            value,
            case_sensitivity,
            never_matches,
        } => {
            if *never_matches {
                return false;
            }
            let Some(elem_value) = doc.attr(node, local_name.0.as_ref()) else {
                return false;
            };
            let case = case_sensitivity.to_unconditional(true);
            operator.eval_str(elem_value, value.as_ref(), case)
        }
        Component::AttributeOther(attr) => {
            // Namespace-qualified attribute selectors: we treat every
            // document as HTML and ignore namespace prefixes.
            if attr.never_matches {
                return false;
            }
            let Some(elem_value) = doc.attr(node, attr.local_name.0.as_ref()) else {
                return false;
            };
            match &attr.operation {
                parcel_selectors::attr::ParsedAttrSelectorOperation::Exists => true,
                parcel_selectors::attr::ParsedAttrSelectorOperation::WithValue {
                    operator,
                    case_sensitivity,
                    expected_value,
                } => {
                    let case = case_sensitivity.to_unconditional(true);
                    operator.eval_str(elem_value, expected_value.as_ref(), case)
                }
            }
        }
        Component::Root => !doc.parent(node).is_some_and(|p| doc.is_element(p)),
        Component::Empty => doc
            .node(node)
            .children
            .iter()
            .all(|&c| matches!(doc.node(c).data, brows12_html::NodeData::Comment(_))),
        Component::Nth(nth) => matches_nth(doc, node, *nth),
        Component::NthOf(nth_of) => {
            let nth = *nth_of.nth_data();
            let inner = nth_of.selectors();
            count_nth(doc, node, nth, |sib| {
                sib == node || inner.iter().any(|sel| matches_selector(doc, sib, sel))
            })
        }
        Component::Is(list) | Component::Where(list) => {
            list.iter().any(|sel| matches_selector(doc, node, sel))
        }
        Component::Negation(list) => !list.iter().any(|sel| matches_selector(doc, node, sel)),
        Component::NonTSPseudoClass(pc) => pseudo_class_matches(doc, node, pc),
        // Not supported in v1:
        Component::Scope
        | Component::Slotted(_)
        | Component::Part(_)
        | Component::Host(_)
        | Component::Has(_)
        | Component::Any(..)
        | Component::Nesting
        | Component::PseudoElement(_)
        | Component::Combinator(_) => false,
    }
}

fn doc_is_html(_doc: &Document, _node: NodeId) -> bool {
    // Brows12 v1 only parses text/html documents.
    true
}

fn matches_nth(doc: &Document, node: NodeId, nth: NthSelectorData) -> bool {
    if nth.ty == NthType::Col || nth.ty == NthType::LastCol {
        return false;
    }
    count_nth(doc, node, nth, |_sib| true)
}

/// Shared nth engine: indexes element `node` among its element siblings
/// (optionally filtered) and evaluates an+b according to the nth type.
fn count_nth(
    doc: &Document,
    node: NodeId,
    nth: NthSelectorData,
    matches_filter: impl Fn(NodeId) -> bool,
) -> bool {
    let parent = match doc.parent(node) {
        Some(p) => p,
        None => return false, // :nth-child never matches the root
    };
    let of_type = matches!(nth.ty, NthType::OfType | NthType::LastOfType | NthType::OnlyOfType);
    let tag = doc.local_name(node);
    let sibs: Vec<NodeId> = doc
        .child_elements(parent)
        .into_iter()
        .filter(|&s| matches_filter(s) && (!of_type || doc.local_name(s) == tag))
        .collect();
    let total = sibs.len();
    let idx = match sibs.iter().position(|&s| s == node) {
        Some(i) => i as i32 + 1,
        None => return false,
    };
    let (a, b) = (nth.a, nth.b);
    let n_ok = if a == 0 {
        idx == b
    } else {
        let diff = idx - b;
        diff % a == 0 && diff / a >= 0
    };
    match nth.ty {
        NthType::Child | NthType::OfType => n_ok,
        NthType::LastChild | NthType::LastOfType => {
            let from_end = (total as i32 - idx) + 1;
            if a == 0 {
                from_end == b
            } else {
                let diff = from_end - b;
                diff % a == 0 && diff / a >= 0
            }
        }
        NthType::OnlyChild | NthType::OnlyOfType => total == 1,
        NthType::Col | NthType::LastCol => false,
    }
}

fn pseudo_class_matches(doc: &Document, node: NodeId, pc: &PseudoClass) -> bool {
    use PseudoClass as P;
    match pc {
        P::Link | P::AnyLink(_) => {
            matches!(doc.local_name(node), "a" | "area") && doc.attr(node, "href").is_some()
        }
        P::Visited => false, // never matches: privacy (history sniffing)
        P::Defined => true,
        P::Disabled | P::Enabled => {
            let tag = doc.local_name(node);
            let form = matches!(
                tag,
                "input" | "select" | "textarea" | "button" | "optgroup" | "option" | "fieldset"
            );
            if !form {
                return false;
            }
            let disabled = doc.attr(node, "disabled").is_some();
            match pc {
                P::Disabled => disabled,
                _ => !disabled && doc.attr(node, "fieldset").is_none(),
            }
        }
        P::Checked => {
            doc.attr(node, "checked").is_some()
                && matches!(doc.attr(node, "type"), Some("checkbox") | Some("radio"))
        }
        P::Hover | P::Active | P::Focus | P::FocusVisible | P::FocusWithin => false,
        _ => false,
    }
}
