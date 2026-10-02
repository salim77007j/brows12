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
        match in_string {
            Some(q) => {
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
            None => {}
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
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_') {
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
                    } else if c == b';' && d2 == 0 {
                        break;
                    } else if c == b'}' && d2 == 0 {
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
        let prev_ok = i == 0
            || !(chars[i - 1].is_ascii_alphabetic() || chars[i - 1] == '-');
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
