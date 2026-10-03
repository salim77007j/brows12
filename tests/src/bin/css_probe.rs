//! Probe: parse the fetched Wikipedia skin stylesheet with brows12-css and
//! report rule counts + timing, to isolate a parse regression.
//!
//! Run: cargo run -p brows12-tests --bin css_probe
use std::time::Instant;

fn main() {
    for path in ["/tmp/wiki_skin.css", "/tmp/wiki_site.css"] {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                println!("{path}: READ FAIL {e}");
                continue;
            }
        };
        let t0 = Instant::now();
        match brows12_css::Stylesheet::parse(&src, brows12_css::Origin::Author) {
            Ok(sheet) => {
                println!(
                    "{path}: {} bytes -> {} rules in {:.2}s",
                    src.len(),
                    sheet.rules.len(),
                    t0.elapsed().as_secs_f32()
                );
            }
            Err(e) => println!("{path}: PARSE FAIL {e:?} after {:.2}s", t0.elapsed().as_secs_f32()),
        }
    }
}
