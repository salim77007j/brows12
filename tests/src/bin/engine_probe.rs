//! Probe: load the CORRECT Wikipedia HTML through the full engine pipeline
//! (Tab::load_url_from_string with the real URL so stylesheets+scripts run),
//! then report the PNG size — isolating script-mutation vs network variant.
use std::time::Instant;

fn main() {
    let url = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
    let _ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
    let html = std::fs::read_to_string("/tmp/wiki_article.html").unwrap();
    println!("loaded {} bytes from /tmp/wiki_article.html", html.len());

    let t0 = Instant::now();
    let engine = brows12_engine::Engine::new(Default::default());
    let tab = engine.tab();
    // Strip <script> tags to test the no-JS path first.
    let no_js = if std::env::args().any(|a| a == "--no-js") {
        let mut s = html;
        while let Some(p) = s.find("<script") {
            let start = p;
            let end = s[start..].find("</script>").map(|e| start + e + 9);
            match end {
                Some(e) => s.replace_range(start..e, ""),
                None => break,
            }
        }
        s
    } else {
        html
    };
    tab.load_url_from_string(&no_js, url).unwrap();
    let frame = tab.frame().unwrap();
    let png = frame.encode_png().unwrap();
    std::fs::write("/tmp/engine_probe.png", &png).unwrap();
    println!(
        "--no-js={:?} png_bytes={} took={:.1}s",
        std::env::args().any(|a| a == "--no-js"),
        png.len(),
        t0.elapsed().as_secs_f32()
    );
}
