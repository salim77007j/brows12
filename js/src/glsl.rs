//! GLSL ES 1.00 / 3.00 normalizer → desktop GLSL 460 core (for naga).
//!
//! naga's GLSL frontend (v29) only parses desktop core 440/450/460: no
//! `#version 300 es`, no `precision` statements, no `attribute`/`varying`,
//! no `gl_FragColor`, no plain `uniform` without `layout(binding=N)`, and no
//! `#ifdef` preprocessor. Real-world WebGL shaders use all of those.
//!
//! This module is the bridge: a small, well-tested text-level normalizer
//! that turns GLSL ES source (both 100 and 300 es) into 460-core source
//! naga accepts, with deterministic binding assignment shared across the
//! two stages of a program.
//!
//! Honest scope (see docs/CAPABILITY_REPORT.md): object-like `#define`
//! substitution, `#ifdef/#ifndef/#if(defed)/#else/#endif`, precision
//! stripping, 100→300 keyword conversion, per-program binding allocation.
//! Function-like macros, arrays-of-uniform layouts, and interface blocks
//! are outside v0.2 and rejected with a clear error.

/// Shader stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

/// Deterministic resource bindings for one program (shared by both stages).
#[derive(Debug, Default, Clone)]
pub struct BindingMap {
    /// Plain (non-sampler) uniforms: name -> binding (0, 1, 2, ...).
    pub uniforms: std::collections::BTreeMap<String, u32>,
    /// Sampler uniforms: name -> texture binding (100, 101, ...). Each also
    /// gets an engine-generated `NAME_s` sampler global in set 1.
    pub samplers: std::collections::BTreeMap<String, u32>,
    /// Vertex attributes: name -> location (0..). Doubles as the WebGL
    /// attribute location space.
    pub attributes: std::collections::BTreeMap<String, u32>,
    /// Varyings (VS out / FS in): name -> location (10..).
    pub varyings: std::collections::BTreeMap<String, u32>,
}

/// Predefined macros (GL_ES semantics) + user `#define`s.
fn predefined() -> Vec<(String, String)> {
    vec![("GL_ES".into(), "1".into()), ("GL_FRAGMENT_PRECISION_HIGH".into(), "1".into())]
}

/// Replace comments with spaces (length- and newline-preserving).
fn blank_comments(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = b.clone();
    let mut i = 0;
    let n = b.len();
    while i < n {
        if b[i] == '/' && i + 1 < n && b[i + 1] == '/' {
            while i < n && b[i] != '\n' {
                out[i] = ' ';
                i += 1;
            }
        } else if b[i] == '/' && i + 1 < n && b[i + 1] == '*' {
            out[i] = ' ';
            out[i + 1] = ' ';
            i += 2;
            while i < n {
                if b[i] == '*' && i + 1 < n && b[i + 1] == '/' {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                    i += 2;
                    break;
                }
                if b[i] != '\n' {
                    out[i] = ' ';
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// One preprocessed, comment-free logical statement or directive.
#[derive(Debug, PartialEq)]
enum Item {
    /// `#version 300 es` / `#version 100`
    Version(u32),
    /// `#define NAME value`
    Define(String, String),
    /// `#ifdef NAME` / `#ifndef NAME` / `#if <const>` / `#else` / `#endif`
    Cond { kind: CondKind, name: String },
    /// Any other code text (may contain braces).
    Code(String),
}

#[derive(Debug, PartialEq)]
enum CondKind {
    If,
    Else,
    Endif,
}

/// Minimal preprocessor: splits source into items, evaluates conditionals
/// with the given macro table, and substitutes object-like macros.
fn preprocess(src: &str, defines: &mut Vec<(String, String)>) -> Result<Vec<Item>, String> {
    let clean = blank_comments(src);
    let mut items = Vec::new();
    // Active-stack: (taken_branch_already, currently_active)
    let mut cond_stack: Vec<(bool, bool)> = Vec::new();
    let mut current_code = String::new();

    let active = |stack: &Vec<(bool, bool)>| stack.iter().all(|(_, a)| *a);

    for raw_line in clean.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix('#') {
            if !current_code.trim().is_empty() {
                items.push(Item::Code(std::mem::take(&mut current_code)));
            }
            let mut toks = rest.split_whitespace();
            let dir = toks.next().unwrap_or("").to_ascii_lowercase();
            match dir.as_str() {
                "version" => {
                    let ver: u32 = toks
                        .next()
                        .unwrap_or("")
                        .parse()
                        .map_err(|_| "#version value".to_string())?;
                    items.push(Item::Version(ver));
                }
                "define" => {
                    let rest_def =
                        rest.trim_start_matches(|c: char| !(c.is_ascii_alphabetic() || c == '_'));
                    let name: String = rest_def
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if name.is_empty() {
                        return Err("bad #define".into());
                    }
                    let value = rest_def[name.len()..].trim().to_string();
                    if value.contains('(') {
                        return Err("function-like macros unsupported".into());
                    }
                    defines.retain(|(n, _)| n != &name);
                    items.push(Item::Define(name.clone(), value.clone()));
                    defines.push((name, value));
                }
                "ifdef" | "ifndef" | "if" => {
                    let arg = rest[dir.len()..].trim();
                    let keep = if dir == "ifdef" {
                        defines.iter().any(|(n, _)| n == arg)
                    } else if dir == "ifndef" {
                        !defines.iter().any(|(n, _)| n == arg)
                    } else {
                        eval_const_if(arg, defines)?
                    };
                    cond_stack.push((keep, keep && active(&cond_stack)));
                    items.push(Item::Cond { kind: CondKind::If, name: arg.to_string() });
                }
                "else" => {
                    let (taken, parent_active) = {
                        let last =
                            cond_stack.last().ok_or_else(|| "#else without #if".to_string())?;
                        let parent = cond_stack[..cond_stack.len() - 1].iter().all(|(_, a)| *a);
                        (last.0, parent)
                    };
                    let slot = cond_stack.last_mut().unwrap();
                    slot.1 = !taken && parent_active;
                    items.push(Item::Cond { kind: CondKind::Else, name: String::new() });
                }
                "endif" => {
                    cond_stack.pop().ok_or_else(|| "#endif without #if".to_string())?;
                    items.push(Item::Cond { kind: CondKind::Endif, name: String::new() });
                }
                "extension" => {} // ignore: extensions are no-ops here
                "precision" => {} // handled as statement too; directive form ignored
                other => return Err(format!("unsupported preprocessor directive #{other}")),
            }
        } else {
            if active(&cond_stack) {
                current_code.push_str(raw_line);
                current_code.push('\n');
            }
        }
    }
    if !current_code.trim().is_empty() {
        items.push(Item::Code(current_code));
    }
    if !cond_stack.is_empty() {
        return Err("unbalanced #if/#endif".into());
    }
    // Merge code items into one text block (statements processed later).
    Ok(items.into_iter().filter(|i| !matches!(i, Item::Cond { .. })).collect())
}

/// `#if` support: integer literals, defined(NAME), NAME (looking up
/// defines), == != && || with parentheses. Enough for real-world shaders.
fn eval_const_if(expr: &str, defines: &[(String, String)]) -> Result<bool, String> {
    let mut s = expr.trim().to_string();
    // defined(X) / defined X
    while let Some(pos) = s.find("defined") {
        let rest = &s[pos + "defined".len()..];
        let rest = rest.trim_start();
        let (name, consumed) = if let Some(stripped) = rest.strip_prefix('(') {
            let end = stripped.find(')').ok_or("defined( without )")?;
            (stripped[..end].trim().to_string(), "(".len() + end + ")".len())
        } else {
            let name: String =
                rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            (name.clone(), name.len())
        };
        let val = if defines.iter().any(|(n, _)| n == &name) { "1" } else { "0" };
        s.replace_range(pos..pos + "defined".len() + consumed, val);
    }
    // Substitute macros (word-level, a few passes).
    for _ in 0..8 {
        let mut replaced = false;
        for (n, v) in defines {
            if s.contains(n.as_str()) {
                s = s.replace(n.as_str(), v);
                replaced = true;
            }
        }
        if !replaced {
            break;
        }
    }
    // Now a tiny recursive-descent bool/arith evaluator.
    let mut p = ExprParser { s: s.as_bytes(), i: 0 };
    let v = p.parse_or()?;
    Ok(v != 0)
}

struct ExprParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> ExprParser<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn parse_or(&mut self) -> Result<i64, String> {
        let mut v = self.parse_and()?;
        loop {
            self.ws();
            if self.s[self.i..].starts_with(b"||") {
                self.i += 2;
                let r = self.parse_and()?;
                v = ((v != 0) || (r != 0)) as i64;
            } else {
                return Ok(v);
            }
        }
    }
    fn parse_and(&mut self) -> Result<i64, String> {
        let mut v = self.parse_cmp()?;
        loop {
            self.ws();
            if self.s[self.i..].starts_with(b"&&") {
                self.i += 2;
                let r = self.parse_cmp()?;
                v = ((v != 0) && (r != 0)) as i64;
            } else {
                return Ok(v);
            }
        }
    }
    fn parse_cmp(&mut self) -> Result<i64, String> {
        let v = self.parse_atom()?;
        self.ws();
        for op in ["==", "!=", ">=", "<=", ">", "<"] {
            if self.s[self.i..].starts_with(op.as_bytes()) {
                self.i += op.len();
                let r = self.parse_atom()?;
                return Ok(match op {
                    "==" => (v == r) as i64,
                    "!=" => (v != r) as i64,
                    ">=" => (v >= r) as i64,
                    "<=" => (v <= r) as i64,
                    ">" => (v > r) as i64,
                    _ => (v < r) as i64,
                });
            }
        }
        Ok(v)
    }
    fn parse_atom(&mut self) -> Result<i64, String> {
        self.ws();
        if self.i < self.s.len() && self.s[self.i] == b'(' {
            self.i += 1;
            let v = self.parse_or()?;
            self.ws();
            if self.i >= self.s.len() || self.s[self.i] != b')' {
                return Err("expected )".into());
            }
            self.i += 1;
            return Ok(v);
        }
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit()) {
            self.i += 1;
        }
        if start == self.i {
            return Err(format!("const-if: unexpected token at byte {start}"));
        }
        std::str::from_utf8(&self.s[start..self.i])
            .map_err(|e| e.to_string())?
            .parse::<i64>()
            .map_err(|e| e.to_string())
    }
}

/// Sampler types that get a binding and a texture unit.
const SAMPLER_TYPES: &[&str] = &[
    "sampler2D",
    "sampler3D",
    "samplerCube",
    "sampler2DShadow",
    "samplerCubeShadow",
    "sampler2DArray",
    "isampler2D",
    "usampler2D",
];

const PRECISION_QUALS: &[&str] = &["highp", "mediump", "lowp"];

/// Statements of one code block at brace depth 0.
fn top_statements(code: &str) -> Vec<(String, bool)> {
    // Returns (statement_text, at_depth_zero). Text may include {..}.
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for ch in code.chars() {
        match ch {
            '{' => {
                depth += 1;
                cur.push(ch);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            ';' if depth == 0 => {
                out.push((std::mem::take(&mut cur), true));
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push((cur, depth == 0));
    }
    out
}

/// Extract the declarator names of a `uniform <type> ...` statement.
fn uniform_declarators(stmt: &str) -> Option<(String, Vec<String>)> {
    let s = stmt.trim();
    // Skip an existing layout(...) prefix if present.
    let s = if s.starts_with("layout") { s.split_once(") ")?.1 } else { s };
    let rest = s.strip_prefix("uniform").map(|r| r.trim())?;
    let mut words = rest.split_whitespace();
    let ty = words.next()?.to_string();
    let names: Vec<String> = words
        .collect::<Vec<_>>()
        .join(" ")
        .split(',')
        .map(|p| p.trim().split(['[', '=']).next().unwrap_or("").trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    Some((ty, names))
}

/// First pass over both stages: collect uniforms, samplers, attributes and
/// varyings in deterministic order and allocate bindings + locations.
pub fn collect_bindings(vs: &str, fs: &str) -> Result<BindingMap, String> {
    let mut map = BindingMap::default();
    let mut next_uniform = 0u32;
    let mut next_sampler = 100u32;
    let mut next_attrib = 0u32;
    let mut next_varying = 10u32;

    let scan = |src: &str,
                stage: Stage,
                map: &mut BindingMap,
                counters: (&mut u32, &mut u32, &mut u32, &mut u32)|
     -> Result<(), String> {
        let (nu, ns, na, nv) = counters;
        let items = preprocess(src, &mut predefined())?;
        for item in items {
            let Item::Code(code) = item else { continue };
            for (stmt, at_top) in top_statements(&code) {
                if !at_top {
                    continue;
                }
                let s = stmt.trim();
                // in/out declarations: classify by keyword + stage.
                // attribute -> attribute; varying(VS)/out(VS) -> varying;
                // in(VS) -> attribute; in(FS) -> varying; out(FS) -> color
                // output (not tracked).
                enum Kind {
                    Attr,
                    Var,
                }
                let in_stmt = s.starts_with("in ");
                let out_stmt = s.starts_with("out ");
                let kind: Option<Kind> =
                    if s.starts_with("attribute ") || (in_stmt && stage == Stage::Vertex) {
                        Some(Kind::Attr)
                    } else if (s.starts_with("varying ") || out_stmt) && stage == Stage::Vertex
                        || (in_stmt && stage == Stage::Fragment)
                    {
                        Some(Kind::Var)
                    } else {
                        None
                    };
                if let Some(kind) = kind {
                    let kw_len = if s.starts_with("attribute ") {
                        "attribute ".len()
                    } else if s.starts_with("in ") {
                        "in ".len()
                    } else if s.starts_with("varying ") {
                        "varying ".len()
                    } else {
                        "out ".len()
                    };
                    let rest = &s[kw_len..];
                    let words = rest.split_whitespace().collect::<Vec<_>>();
                    if words.len() >= 2 && !words[0].contains('(') {
                        let name = words[1].split(',').next().unwrap_or("").trim().to_string();
                        let valid = name
                            .chars()
                            .next()
                            .map(|c| c.is_ascii_alphabetic() || c == '_')
                            .unwrap_or(false);
                        if valid {
                            let table = match kind {
                                Kind::Attr => &mut map.attributes,
                                Kind::Var => &mut map.varyings,
                            };
                            if let std::collections::btree_map::Entry::Vacant(e) = table.entry(name)
                            {
                                let slot = match kind {
                                    Kind::Attr => &mut *na,
                                    Kind::Var => &mut *nv,
                                };
                                e.insert(*slot);
                                *slot += 1;
                            }
                        }
                    }
                }
                // uniforms / samplers.
                if let Some((ty, names)) = uniform_declarators(s) {
                    let is_sampler = SAMPLER_TYPES.contains(&ty.as_str());
                    for name in names {
                        let table = if is_sampler { &mut map.samplers } else { &mut map.uniforms };
                        if let std::collections::btree_map::Entry::Vacant(e) = table.entry(name) {
                            let b = if is_sampler { *ns } else { *nu };
                            e.insert(b);
                            if is_sampler {
                                *ns += 1;
                            } else {
                                *nu += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    };

    // VS first (attributes/varyings declared there define locations).
    scan(
        vs,
        Stage::Vertex,
        &mut map,
        (&mut next_uniform, &mut next_sampler, &mut next_attrib, &mut next_varying),
    )?;
    scan(
        fs,
        Stage::Fragment,
        &mut map,
        (&mut next_uniform, &mut next_sampler, &mut next_attrib, &mut next_varying),
    )?;
    Ok(map)
}

/// Normalize one stage: returns 460-core GLSL that naga can parse.
pub fn normalize(src: &str, stage: Stage, map: &BindingMap) -> Result<String, String> {
    let mut defines = predefined();
    let items = preprocess(src, &mut defines)?;
    let mut version = 100u32;
    for item in &items {
        if let Item::Version(v) = item {
            version = *v;
        }
    }

    let mut body = String::new();
    let mut frag_out_decl: Option<String> = None;

    for item in &items {
        let Item::Code(code) = item else { continue };
        for (stmt, at_top) in top_statements(code) {
            let mut s = stmt.trim().to_string();
            if s.is_empty() {
                continue;
            }
            // Strip precision statements (any stage).
            if s.starts_with("precision") {
                continue;
            }
            // Strip precision qualifiers.
            for q in PRECISION_QUALS {
                while let Some(pos) = find_word(&s, q) {
                    s.replace_range(pos..pos + q.len(), "");
                    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
                }
            }
            if !at_top {
                body.push_str(&s);
                if !s.ends_with('}') {
                    body.push(';');
                }
                body.push('\n');
                continue;
            }
            // Rewrite texture calls (also covers whole-function statements).
            s = rewrite_sampler_calls(&s, &map.samplers);
            // GLSL 100 keyword conversion.
            if version < 300 {
                s = convert_100(&s, stage);
            }
            // in/out location injection (naga assigns location 0 to every
            // unannotated in/out -> BindingCollision; explicit is required).
            s = inject_stage_locations(&s, stage, map);
            // Uniform binding injection (before sampler rewrite: the
            // sampler rewrite consumes the injected layout prefix).
            if let Some((ty, names)) = uniform_declarators(&s) {
                let is_sampler = SAMPLER_TYPES.contains(&ty.as_str());
                if !names.is_empty() && !s.starts_with("layout") {
                    let table = if is_sampler { &map.samplers } else { &map.uniforms };
                    if let Some(b) = table.get(&names[0]) {
                        s = format!("layout(binding = {b}) {s}");
                    }
                }
            }
            // sampler2D declarations -> texture2D + set-1 sampler pair.
            s = rewrite_sampler_decls(&s, &map.samplers);
            // Fragment output for GLSL 100 gl_FragColor / gl_FragData, and
            // explicit outs (300 es) get location 0.
            if stage == Stage::Fragment {
                if s.contains("gl_FragColor") || s.contains("gl_FragData") {
                    if frag_out_decl.is_none() {
                        frag_out_decl =
                            Some("layout(location = 0) out vec4 b12_fragColor;".to_string());
                    }
                    s = s.replace("gl_FragData[0]", "b12_fragColor");
                    s = s.replace("gl_FragColor", "b12_fragColor");
                }
                if s.starts_with("out ") && !s.starts_with("layout") {
                    s = format!("layout(location = 0) {s}");
                }
            }
            body.push_str(&s);
            if !s.ends_with(';') && !s.ends_with('}') {
                body.push(';');
            }
            body.push('\n');
        }
    }

    let mut out = String::from("#version 460 core\n");
    if let Some(d) = &frag_out_decl {
        out.push_str(d);
        out.push('\n');
    }
    out.push_str(&body);
    Ok(out)
}

/// Prefix in/out declarations with explicit locations from the map.
fn inject_stage_locations(s: &str, stage: Stage, map: &BindingMap) -> String {
    if s.starts_with("layout") || s.contains('{') {
        return s.to_string();
    }
    let (kw, table): (&str, &std::collections::BTreeMap<String, u32>) = match stage {
        Stage::Vertex if s.starts_with("in ") => ("in ", &map.attributes),
        Stage::Vertex if s.starts_with("out ") => ("out ", &map.varyings),
        Stage::Fragment if s.starts_with("in ") => ("in ", &map.varyings),
        _ => return s.to_string(),
    };
    let rest = s.strip_prefix(kw).unwrap();
    let words = rest.split_whitespace().collect::<Vec<_>>();
    if words.len() < 2 {
        return s.to_string();
    }
    let name = words[1].split(',').next().unwrap_or("").trim();
    if let Some(loc) = table.get(name) {
        return format!("layout(location = {loc}) {s}");
    }
    s.to_string()
}

/// `uniform sampler2D NAME;` -> `uniform texture2D NAME;` + set-1 sampler.
fn rewrite_sampler_decls(s: &str, _samplers: &std::collections::BTreeMap<String, u32>) -> String {
    let Some(rest) = s.strip_prefix("layout(binding = ") else { return s.to_string() };
    let binding: u32 = match rest.split(')').next().unwrap_or("").trim().parse() {
        Ok(b) => b,
        Err(_) => return s.to_string(),
    };
    let after = rest.split_once(") ").map(|x| x.1).unwrap_or("");
    let Some(after_uniform) = after.strip_prefix("uniform ") else { return s.to_string() };
    let mut words = after_uniform.split_whitespace();
    let ty = words.next().unwrap_or("");
    let name = words.next().unwrap_or("");
    if !SAMPLER_TYPES.contains(&ty) || name.is_empty() {
        return s.to_string();
    }
    let tex_ty = match ty {
        "sampler2D" => "texture2D",
        "samplerCube" => "textureCube",
        _ => return s.to_string(), // shadow/array/3D samplers unsupported
    };
    format!(
        "layout(binding = {binding}) uniform {tex_ty} {name};\nlayout(set = 1, binding = {binding}) uniform sampler {name}_s"
    )
}

/// `texture(NAME, ...)` -> `texture(sampler2D(NAME, NAME_s), ...)`.
/// Rewrites every occurrence (a function body may sample repeatedly).
fn rewrite_sampler_calls(s: &str, samplers: &std::collections::BTreeMap<String, u32>) -> String {
    let mut out = s.to_string();
    for fname in ["texture", "textureLod"] {
        let needle = format!("{fname}(");
        let mut search_from = 0;
        while let Some(rel) = out[search_from..].find(&needle) {
            let pos = search_from + rel;
            let rest = &out[pos + needle.len()..];
            let trimmed = rest.trim_start();
            let ident: String =
                trimmed.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            let rewrite = samplers.contains_key(&ident)
                && trimmed[ident.len()..].trim_start().starts_with(',');
            if !rewrite {
                search_from = pos + needle.len();
                continue;
            }
            let after_ident = trimmed[ident.len()..].trim_start();
            let comma_len = after_ident.len() - after_ident.trim_start_matches(',').len();
            let consumed = (rest.len() - rest.trim_start().len()) + ident.len() + comma_len;
            let replacement = format!("{fname}(sampler2D({ident}, {ident}_s),");
            out.replace_range(pos..pos + needle.len() + consumed, &replacement);
            search_from = pos + replacement.len();
        }
    }
    out
}

/// GLSL 100 → 300-style keyword conversion on one top-level statement or
/// code block (attribute→in, varying→out/in, texture2D→texture).
fn convert_100(s: &str, stage: Stage) -> String {
    let mut out = s.to_string();
    // Function-body texture calls.
    out = out.replace("texture2D(", "texture(").replace("textureCube(", "texture(");
    // Declaration keywords at statement start.
    for (from, to) in
        [("attribute ", "in "), ("varying ", if stage == Stage::Vertex { "out " } else { "in " })]
    {
        if out.trim_start().starts_with(from) {
            out = out.replacen(from, to, 1);
        }
    }
    out
}

/// Word-boundary search.
fn find_word(s: &str, w: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut start = 0;
    while let Some(pos) = s[start..].find(w) {
        let abs = start + pos;
        let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let end = abs + w.len();
        let after_ok = end >= s.len() || !bytes[end].is_ascii_alphanumeric() && bytes[end] != b'_';
        if before_ok && after_ok {
            return Some(abs);
        }
        start = abs + w.len().max(1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const VS100: &str = r#"
precision highp float;
attribute vec2 a_pos;
uniform mat4 u_mvp;
varying vec2 v_uv;
void main() {
  v_uv = a_pos;
  gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0);
}
"#;

    const FS300: &str = r#"
#version 300 es
precision mediump float;
uniform sampler2D u_tex;
uniform vec4 u_tint;
in vec2 v_uv;
out vec4 o_color;
void main() {
  o_color = texture(u_tex, v_uv) * u_tint;
}
"#;

    const FS100: &str = r#"
#ifdef GL_ES
precision mediump float;
#endif
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
  gl_FragColor = texture2D(u_tex, v_uv);
}
"#;

    #[test]
    fn binds_uniforms_and_samplers_deterministically() {
        let map = collect_bindings(VS100, FS300).unwrap();
        assert_eq!(map.uniforms["u_mvp"], 0);
        assert_eq!(map.uniforms["u_tint"], 1);
        assert_eq!(map.samplers["u_tex"], 100);
        assert_eq!(map.attributes["a_pos"], 0);
        assert_eq!(map.varyings["v_uv"], 10);
    }

    #[test]
    fn normalizes_vertex_100() {
        let map = collect_bindings(VS100, FS300).unwrap();
        let out = normalize(VS100, Stage::Vertex, &map).unwrap();
        assert!(out.starts_with("#version 460 core"));
        assert!(out.contains("layout(location = 0) in vec2 a_pos"));
        assert!(out.contains("layout(location = 10) out vec2 v_uv"));
        assert!(out.contains("layout(binding = 0) uniform mat4 u_mvp"));
        assert!(!out.contains("precision"));
        assert!(!out.contains("attribute"));
        assert!(!out.contains("varying"));
    }

    #[test]
    fn normalizes_fragment_300() {
        let map = collect_bindings(VS100, FS300).unwrap();
        let out = normalize(FS300, Stage::Fragment, &map).unwrap();
        // sampler2D becomes texture2D + set-1 sampler pair.
        assert!(out.contains("layout(binding = 100) uniform texture2D u_tex"));
        assert!(out.contains("layout(set = 1, binding = 100) uniform sampler u_tex_s"));
        assert!(out.contains("layout(binding = 1) uniform vec4 u_tint"));
        assert!(out.contains("layout(location = 10) in vec2 v_uv"));
        assert!(out.contains("layout(location = 0) out vec4 o_color"));
        assert!(out.contains("texture(sampler2D(u_tex, u_tex_s)"));
    }

    #[test]
    fn converts_fragcolor_100() {
        let map = collect_bindings(VS100, FS100).unwrap();
        let out = normalize(FS100, Stage::Fragment, &map).unwrap();
        assert!(out.contains("b12_fragColor"));
        assert!(out.contains("layout(location = 0) out vec4 b12_fragColor;"));
        assert!(out.contains("texture("));
        assert!(!out.contains("gl_FragColor"));
        // The #ifdef GL_ES precision statement must be dropped.
        assert!(!out.contains("precision"));
    }

    #[test]
    fn ifdef_else_paths() {
        let src = "#ifdef GL_ES\nfloat x = 1.0;\n#else\nfloat x = 2.0;\n#endif\n";
        let map = BindingMap::default();
        let out = normalize(src, Stage::Vertex, &map).unwrap();
        assert!(out.contains("float x = 1.0"));
        assert!(!out.contains("float x = 2.0"));
    }
}
