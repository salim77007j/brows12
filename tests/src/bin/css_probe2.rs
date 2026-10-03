//! Probe: A/B the engine's parse_with_env pipeline vs plain parse on the
//! fetched Wikipedia stylesheets.
//!
//! Run: cargo run -p brows12-tests --bin css_probe2
use std::time::Instant;

use brows12_css::vartext::ScopeEnv;

fn probe(path: &str, env: ScopeEnv) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            println!("{path}: READ FAIL {e}");
            return;
        }
    };
    let t0 = Instant::now();
    match brows12_css::Stylesheet::parse(&src, brows12_css::Origin::Author) {
        Ok(s) => println!(
            "{path}: plain     -> {} rules, {} keyframes, {:.2}s",
            s.rules.len(),
            s.keyframes.len(),
            t0.elapsed().as_secs_f32()
        ),
        Err(e) => println!("{path}: plain PARSE FAIL {e:?}"),
    }
    let t0 = Instant::now();
    match brows12_css::Stylesheet::parse_with_env(&src, brows12_css::Origin::Author, env) {
        Ok(s) => println!(
            "{path}: with_env  -> {} rules, {} keyframes, {} custom_defs, {:.2}s",
            s.rules.len(),
            s.keyframes.len(),
            s.custom_defs.len(),
            t0.elapsed().as_secs_f32()
        ),
        Err(e) => println!("{path}: with_env PARSE FAIL {e:?}"),
    }
}

fn main() {
    let env = ScopeEnv { dark_preferred: false, viewport_width: 1280.0, viewport_height: 800.0 };
    probe("/tmp/wiki_skin.css", env);
    probe("/tmp/wiki_site.css", env);
}
