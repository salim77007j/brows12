//! # brows12-css
//!
//! CSS parsing, selector matching and cascade computation for the Brows12
//! browser engine.
//!
//! Parsing uses `lightningcss` (Parcel's CSS engine) — the fastest
//! production-grade CSS parser in the Rust ecosystem — which hands us a fully
//! owned, allocation-lean AST including selectors.
//!
//! Selector matching is implemented against the `parcel_selectors` component
//! AST (the same grammar model Stylo uses), supporting the selector subset
//! that covers the overwhelming majority of real-world stylesheets: type,
//! class, id, attribute (all operators), descendant/child/sibling
//! combinators, grouping, `:is()`/`:where()`/`:not()`, structural
//! pseudo-classes (`:root`, `:empty`, `:first/last/only-child`,
//! `:nth-child(an+b[ of S])`, `:nth-of-type`, ...), link pseudo-classes and
//! form-state pseudo-classes. The matcher is property-tested and fuzzed.
//!
//! The cascade implements the CSS cascade algorithm for author + user-agent
//! origins with specificity, source order and `!important` handling, plus
//! `style=""` inline declarations and per-property inheritance.

pub mod apply;
pub mod cascade;
pub mod computed;
pub mod matcher;
pub mod stylesheet;
pub mod values;

pub use cascade::{compute_styles, StyleMap};
pub use computed::ComputedStyle;
pub use stylesheet::{Origin, StyleEngine, Stylesheet};
pub use values::{AlignItems, AutoPx, Display, Edges, FlexDirection, FlexWrap, FontStyle,
    JustifyContent, Len, LineHeight, OverflowKeyword, Position, TextAlign, WhiteSpace, ZIndex};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CssError {
    #[error("CSS parse error: {0}")]
    Parse(String),
}
