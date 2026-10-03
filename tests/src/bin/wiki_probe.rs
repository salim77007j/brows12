//! Probe: replicate the engine pipeline (fetch → parse → stylesheets →
//! cascade → layout) on the live Wikipedia page and dump the box tree of
//! the top three levels with tag/id/class, display, and rect — to locate
//! the body-height collapse.
//!
//! Run: cargo run -p brows12-tests --bin wiki_probe
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brows12_css::vartext::ScopeEnv;
use brows12_css::{compute_styles, computed::CascadeCtx, Origin, StyleEngine, Stylesheet};
use brows12_html::parse_document;
use brows12_layout::{compute_layout, TextMeasurer, Viewport};

#[tokio::main]
async fn main() {
    let url = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
    let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
    let client = brows12_net::HttpClient::new(Default::default(), None);
    let mut req = brows12_net::NetRequest::get(url, "https://en.wikipedia.org");
    req.headers.push(("User-Agent".into(), ua.into()));
    let resp = client.send(req).await.expect("html fetch");
    let html = String::from_utf8_lossy(&resp.body).to_string();
    println!("HTML {} bytes", html.len());

    let doc = Arc::new(parse_document(&html));
    let base = url::Url::parse(url).unwrap();

    // Collect stylesheet links exactly like engine/src/tab.rs.
    let mut links = Vec::new();
    doc.visit_all(|n| {
        if doc.is_element(n)
            && doc.local_name(n) == "link"
            && doc.attr(n, "rel").map(|r| r.eq_ignore_ascii_case("stylesheet")).unwrap_or(false)
        {
            if let Some(href) = doc.attr(n, "href") {
                links.push(href.to_string());
            }
        }
    });
    println!("stylesheet links: {}", links.len());
    let env = ScopeEnv { dark_preferred: false, viewport_width: 1280.0, viewport_height: 800.0 };
    let mut sheets = Vec::new();
    for (i, href) in links.iter().enumerate() {
        if i >= 12 {
            break;
        }
        let resolved = match base.join(href) {
            Ok(u) => u.to_string(),
            Err(_) => continue,
        };
        let mut r = brows12_net::NetRequest::get(&resolved, "https://en.wikipedia.org");
        r.headers.push(("User-Agent".into(), ua.into()));
        match client.send(r).await {
            Ok(resp) if resp.is_success() => {
                let css = String::from_utf8_lossy(&resp.body).to_string();
                match Stylesheet::parse_with_env(&css, Origin::Author, env) {
                    Ok(sheet) => {
                        println!("  [{i}] {} -> {} rules", resolved.len(), sheet.rules.len());
                        sheets.push(sheet);
                    }
                    Err(e) => println!("  [{i}] {} PARSE FAIL {e:?}", resolved),
                }
            }
            Ok(resp) => println!("  [{i}] {} status {}", resolved, resp.status),
            Err(e) => println!("  [{i}] {} FETCH ERR {e:?}", resolved),
        }
    }

    // Inline <style> blocks.
    doc.visit_all(|n| {
        if doc.is_element(n) && doc.local_name(n) == "style" {
            let css = doc.text_content(n);
            if let Ok(sheet) = Stylesheet::parse_with_env(&css, Origin::Author, env) {
                sheets.push(sheet);
            }
        }
    });
    println!("total sheets: {}", sheets.len());

    let engine_sheet = StyleEngine::with_author_sheets(&sheets);
    let ctx = CascadeCtx { viewport_width: 1280.0, viewport_height: 800.0, ..Default::default() };
    let styles = compute_styles(&doc, &engine_sheet, &ctx);
    let measurer = TextMeasurer::new(Arc::new(Mutex::new(cosmic_text::FontSystem::new())));
    let mut result = compute_layout(
        &doc,
        &styles,
        Viewport { width: 1280.0, height: 800.0 },
        &measurer,
        &HashMap::new(),
    );

    // Replicate the engine's container-query re-cascade.
    let has_container_rules = engine_sheet.rules().iter().any(|r| !r.containers.is_empty());
    println!("has_container_rules: {has_container_rules}");
    if has_container_rules {
        let sizes: HashMap<brows12_html::NodeId, f32> = styles
            .styles
            .iter()
            .filter(|(_, s)| s.container_type != brows12_css::values::ContainerType::Normal)
            .filter_map(|(n, _)| result.rect(*n).map(|r| (*n, r.width)))
            .collect();
        println!("container sizes: {}", sizes.len());
        let styles2 =
            brows12_css::compute_styles_with_containers(&doc, &engine_sheet, &ctx, &sizes, &styles);
        let body = doc.body().unwrap();
        let d2 = styles2.get(body).map(|s| format!("{:?}", s.display)).unwrap_or_default();
        println!("body display after re-cascade: {d2}");
        result = compute_layout(
            &doc,
            &styles2,
            Viewport { width: 1280.0, height: 800.0 },
            &measurer,
            &HashMap::new(),
        );
        let _ = styles; // keep alive for the dump below (uses first-pass map)
    }

    // Dump top-of-tree boxes.
    let body = doc.body().unwrap();
    // Deep-dump the header subtree (5 levels) — the header is 1220px tall
    // vs Chrome's ~110px; find the box that explodes.
    fn deep(
        doc: &brows12_html::Document,
        styles: &brows12_css::StyleMap,
        result: &brows12_layout::LayoutResult,
        n: brows12_html::NodeId,
        depth: usize,
        max_depth: usize,
    ) {
        if depth > max_depth {
            return;
        }
        let tag = doc.local_name(n).to_string();
        let id = doc.attr(n, "id").unwrap_or("").to_string();
        let cls = doc.attr(n, "class").unwrap_or("").to_string();
        let d = styles.get(n).map(|s| format!("{:?}", s.display)).unwrap_or_default();
        let pos = styles.get(n).map(|s| format!("{:?}", s.position)).unwrap_or_default();
        let r = result
            .rect(n)
            .map(|r| format!("{:.0}x{:.0}@{},{}", r.width, r.height, r.x, r.y))
            .unwrap_or("NORECT".into());
        if doc.is_element(n) && (depth <= 1 || r != "NORECT") {
            println!(
                "{}<{}/{} id={id} cls=[{}] disp={} pos={} {}",
                "  ".repeat(depth),
                tag,
                n.0,
                if cls.len() > 60 { cls[..60].to_string() } else { cls },
                d,
                pos,
                r
            );
        }
        for &c in &doc.node(n).children {
            deep(doc, styles, result, c, depth + 1, max_depth);
        }
    }
    let header = doc.get_elements_by_tag_name("header").into_iter().next().unwrap_or(body);
    deep(&doc, &styles, &result, header, 0, 4);
}
