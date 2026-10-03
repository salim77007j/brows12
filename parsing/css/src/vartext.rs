//! Text-level CSS custom property (var()) handling.
//!
//! lightningcss does not expose per-declaration serialization, so variable
//! resolution happens on the raw stylesheet text before the real parse:
//! 1. `collect_custom_defs` scans for `--name: value` declarations.
//! 2. `substitute_vars_text` rewrites `var(--name, fallback)` references.
//!
//! Scope model (documented in CAPABILITY_REPORT.md): custom properties
//! declared in author stylesheets resolve document-wide (design-token
//! usage on `:root` covers the overwhelming majority of real-world var()
//! usage). Custom properties declared inline (`style="--x: ..."`) resolve
//! per element with inheritance.

use std::collections::HashMap;

/// Scan CSS text for custom property declarations (`--name: value`).
/// Later definitions win, matching source order.
pub fn collect_custom_defs(css: &str, out: &mut HashMap<String, String>) {
    let bytes = css.as_bytes();
    let mut i = 0usize;
    let mut depth = 0i32;
    let mut in_string: Option<u8> = None;
    let mut in_comment = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_comment {
            if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(q) = in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                in_string = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                in_comment = true;
                i += 2;
                continue;
            }
            b'"' | b'\'' => {
                in_string = Some(b);
                i += 1;
                continue;
            }
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        // Custom property declaration start: `--name` immediately followed
        // (after whitespace) by `:`.
        if b == b'-'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'-'
            && depth >= 0
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'-'))
        {
            let name_start = i;
            let mut j = i + 2;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_')
            {
                j += 1;
            }
            let name_end = j;
            // Expect optional whitespace + ':'.
            let mut k = j;
            while k < bytes.len() && (bytes[k] as char).is_whitespace() {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b':' {
                let name = css[name_start..name_end].to_ascii_lowercase();
                // Capture value until ';' or '}' at current depth.
                let mut v = k + 1;
                let vstart = v;
                let mut d2 = 0i32;
                while v < bytes.len() {
                    let c = bytes[v];
                    if c == b'(' {
                        d2 += 1;
                    } else if c == b')' {
                        d2 -= 1;
                    } else if (c == b';' || c == b'}') && d2 == 0 {
                        break;
                    }
                    v += 1;
                }
                let value = css[vstart..v].trim().to_string();
                if !name.is_empty() && !value.is_empty() {
                    out.insert(name, value);
                }
                i = v;
                continue;
            }
            i = j;
            continue;
        }
        i += 1;
    }
}

/// Rewrite `var(--name, fallback)` references in `css` using `vars`.
/// Missing names without fallback become empty values (the declaration
/// is then dropped by the parser — guaranteed-invalid approximation).
pub fn substitute_vars_text(css: &str, vars: &HashMap<String, String>) -> String {
    if !css.contains("var(") {
        return css.to_string();
    }
    // Char-based scan: CSS text is UTF-8 and `var(` handling must not split
    // multi-byte characters (the byte loop previously panicked mid-codepoint).
    let chars: Vec<char> = css.chars().collect();
    let mut out = String::with_capacity(css.len() + 64);
    let mut i = 0usize;
    while i < chars.len() {
        let starts_var = chars[i] == 'v'
            && chars.get(i + 1) == Some(&'a')
            && chars.get(i + 2) == Some(&'r')
            && chars.get(i + 3) == Some(&'(');
        let prev_ok = i == 0 || !(chars[i - 1].is_ascii_alphabetic() || chars[i - 1] == '-');
        if starts_var && prev_ok {
            let start = i + 4;
            let mut depth = 1i32;
            let mut j = start;
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if depth != 0 {
                for c in &chars[i..] {
                    out.push(*c);
                }
                break;
            }
            let inner: String = chars[start..j - 1].iter().collect();
            let (name_part, fallback) = split_var_args(&inner);
            let name = name_part.trim().to_ascii_lowercase();
            match vars.get(&name) {
                Some(resolved) => {
                    let substituted = substitute_vars_text(resolved, vars);
                    out.push_str(&substituted);
                }
                None => match fallback {
                    Some(fb) => {
                        let substituted = substitute_vars_text(fb, vars);
                        out.push_str(substituted.trim());
                    }
                    None => {
                        // Guaranteed-invalid: emit nothing.
                    }
                },
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn split_var_args(inner: &str) -> (&str, Option<&str>) {
    let mut depth = 0i32;
    for (idx, ch) in inner.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return (&inner[..idx], Some(&inner[idx + 1..])),
            _ => {}
        }
    }
    (inner, None)
}

/// Device facts the scoped collector needs to evaluate media-gated token
/// definitions at stylesheet-parse time (the textual substitution pass has
/// no other access to the environment).
#[derive(Debug, Clone, Copy, Default)]
pub struct ScopeEnv {
    pub dark_preferred: bool,
    pub viewport_width: f32,
    pub viewport_height: f32,
}

/// Selector gate: does this selector text define tokens that should join
/// the document-global design-token map? Only root-scoped selectors
/// (`:root`, `html`, `body`, `*` and lists of those) qualify. Qualifier-
/// bearing selectors (`html.dark`, `.theme-x`, `[data-theme]`) declare
/// element-scoped tokens that must NOT leak globally — the bug that made
/// Wikipedia's Vector-2022 dark tokens repaint the whole canvas.
fn selector_is_root_scoped(prelude: &str) -> bool {
    let prelude = prelude.trim().to_ascii_lowercase();
    if prelude.is_empty() {
        return false;
    }
    prelude.split(',').all(|part| {
        let part = part.trim();
        matches!(part, ":root" | "html" | "body" | "*" | ":host")
    })
}

/// Media-gate evaluation for the token collector. Returns `None` when the
/// condition is not one we can decide textually (treated as pass), or
/// `Some(holds)`.
fn eval_media_gate(prelude: &str, env: &ScopeEnv) -> Option<bool> {
    let cond = prelude.trim().to_ascii_lowercase();
    let cond = cond.strip_prefix("@media").unwrap_or(&cond).trim();
    // Media lists are comma-separated OR terms; each term is `and`-joined
    // features. Evaluate what we know; unknown features pass.
    let term_holds = |term: &str| -> bool {
        let mut holds = true;
        for feature in term.split(" and ") {
            let feature = feature.trim();
            let (negated, feature) = match feature.strip_prefix("not ") {
                Some(rest) => (true, rest.trim()),
                None => (false, feature),
            };
            let inner = feature
                .strip_prefix('(')
                .and_then(|f| f.strip_suffix(')'))
                .unwrap_or(feature);
            let decided = if let Some(rest) = inner.strip_prefix("prefers-color-scheme:") {
                let want_dark = rest.trim() == "dark";
                Some(want_dark == env.dark_preferred)
            } else if let Some(rest) = inner.strip_prefix("min-width:") {
                rest.trim()
                    .strip_suffix("px")
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .map(|v| env.viewport_width >= v)
            } else if let Some(rest) = inner.strip_prefix("max-width:") {
                rest.trim()
                    .strip_suffix("px")
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .map(|v| env.viewport_width <= v)
            } else if let Some(rest) = inner.strip_prefix("min-height:") {
                rest.trim()
                    .strip_suffix("px")
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .map(|v| env.viewport_height >= v)
            } else if let Some(rest) = inner.strip_prefix("max-height:") {
                rest.trim()
                    .strip_suffix("px")
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .map(|v| env.viewport_height <= v)
            } else {
                None // unknown feature: pass
            };
            let f = decided.unwrap_or(true);
            holds = holds && (if negated { !f } else { f });
        }
        holds
    };
    Some(cond.split(',').any(term_holds))
}

/// Collect custom property definitions **only** where the document-global
/// scope model is sound: root-scoped selectors, inside at-rule conditions
/// that hold for this device. `--brows-*` reserved names are excluded
/// (they resolve per element during cascade).
///
/// This is the fix for the design-token leak: a dark-theme rule like
/// `html.skin-theme-clientpref-os { --background-color-base: #101418 }`
/// must not repaint pages whose root never matches it.
pub fn collect_root_scoped_defs(
    css: &str,
    out: &mut HashMap<String, String>,
    env: &ScopeEnv,
) {
    let mut fallback = HashMap::new();
    collect_scoped_defs_inner(css, out, &mut fallback, env);
    // Component-scoped tokens fill in names the root scope never defines,
    // so single-definition component tokens still resolve. Names with a
    // root-scoped definition keep the root value (theme-qualified dark
    // variants must not override the light default).
    for (k, v) in fallback {
        out.entry(k).or_insert(v);
    }
}

fn collect_scoped_defs_inner(
    css: &str,
    out: &mut HashMap<String, String>,
    fallback: &mut HashMap<String, String>,
    env: &ScopeEnv,
) {
    let bytes = css.as_bytes();
    let mut i = 0usize;
    // Frames: (is_style_rule, definition_allowed_here, root_scoped, body_start).
    let mut stack: Vec<(bool, bool, bool, usize)> = Vec::new();
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;
    let mut prelude_start = i;
    while i < bytes.len() {
        let b = bytes[i];
        if in_comment {
            if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                in_string = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                in_comment = true;
                i += 2;
                continue;
            }
            b'"' | b'\'' => {
                in_string = Some(b);
                i += 1;
                continue;
            }
            b';' => {
                // Statement at-rule (@import/@charset): discard prelude.
                prelude_start = i + 1;
                i += 1;
                continue;
            }
            b'{' => {
                let prelude = css[prelude_start..i].trim().to_string();
                let frame = if prelude.starts_with('@') {
                    let inherited_allowed = stack.last().map(|(_, a, _, _)| *a).unwrap_or(true);
                    let gate = if prelude.starts_with("@media") {
                        eval_media_gate(&prelude, env).unwrap_or(true)
                    } else if prelude.starts_with("@supports")
                        || prelude.starts_with("@layer")
                        || prelude.starts_with("@container")
                    {
                        // Not textually decidable / non-gating for tokens.
                        true
                    } else {
                        // @keyframes, @font-face, @property, unknown.
                        false
                    };
                    (false, gate && inherited_allowed, false)
                } else {
                    let scoped = selector_is_root_scoped(&prelude);
                    let inherited_allowed = stack.last().map(|(_, a, _, _)| *a).unwrap_or(true);
                    (true, scoped && inherited_allowed, scoped)
                };
                stack.push((frame.0, frame.1, frame.2, i + 1));
                prelude_start = i + 1;
                i += 1;
                continue;
            }
            b'}' => {
                // Declarations of the closing block: harvest the frame's
                // whole body (nested sub-blocks included — a root-scoped
                // rule's tokens must survive nested at-rules like @layer).
                if let Some((is_rule, allowed, scoped_ok, body_start)) = stack.pop() {
                    if is_rule && allowed {
                        let mut defs = HashMap::new();
                        collect_custom_defs(&css[body_start..i], &mut defs);
                        defs.retain(|k, _| !k.starts_with("--brows-"));
                        if scoped_ok {
                            for (k, v) in defs {
                                out.insert(k, v);
                            }
                        } else {
                            for (k, v) in defs {
                                fallback.entry(k).or_insert(v);
                            }
                        }
                    }
                }
                prelude_start = i + 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_root_tokens() {
        let mut defs = HashMap::new();
        collect_custom_defs(":root { --accent: #ff0000; --space: 8px; }", &mut defs);
        assert_eq!(defs.get("--accent").map(|s| s.as_str()), Some("#ff0000"));
        assert_eq!(defs.get("--space").map(|s| s.as_str()), Some("8px"));
    }

    #[test]
    fn substitutes_with_fallback() {
        let mut vars = HashMap::new();
        vars.insert("--a".to_string(), "12px".to_string());
        let out = substitute_vars_text("p { margin: var(--a); top: var(--missing, 4px); }", &vars);
        assert!(out.contains("margin:12px") || out.contains("margin: 12px"), "{out}");
        assert!(out.contains("4px"), "{out}");
    }
}
