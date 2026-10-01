//! Shared fixtures for the Brows12 benchmark suite.

/// Generate a synthetic HTML document with `n` sections.
pub fn html_fixture(n: usize) -> String {
    let mut s = String::with_capacity(n * 200);
    s.push_str("<!DOCTYPE html><html><head><title>Bench</title></head><body>");
    for i in 0..n {
        s.push_str(&format!(
            r#"<section class="item" id="item-{i}"><h2>Heading {i}</h2><p>Paragraph {i} with <a href="/x/{i}">a link</a> and <b>bold</b> <i>italic</i> text.</p><ul><li>one</li><li>two</li><li>three</li></ul></section>"#
        ));
    }
    s.push_str("</body></html>");
    s
}

/// A stylesheet sized for cascade benchmarks.
pub const CSS_FIXTURE: &str = r#"
body { font-size: 16px; color: #222; margin: 0; }
section.item { margin: 12px; padding: 8px 16px; border: 1px solid #ddd; }
section.item h2 { font-size: 1.3em; color: #114488; margin: 0.4em 0; }
section.item p { line-height: 1.5; }
section.item a { color: #0000ee; text-decoration: underline; }
section.item li { margin-left: 16px; }
section.item:nth-child(2n) { background-color: #f8f8f8; }
section#item-7 h2 { font-weight: bold; }
div, span, p, a, ul, ol, li { display: block; }
.hero { background: linear-gradient(#fff, #000); }
@media (max-width: 600px) { section.item { margin: 4px; } }
"#;

/// JS micro-program: deterministic work comparable across engines.
pub const JS_FIB: &str = r#"
function fib(n) { return n < 2 ? n : fib(n-1) + fib(n-2); }
fib(20);
"#;
