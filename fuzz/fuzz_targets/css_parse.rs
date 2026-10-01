#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(sheet) = brows12_css::Stylesheet::parse(s, brows12_css::Origin::Author) {
            // Matching must not panic either: match the first rule against a
            // throwaway document.
            let doc = brows12_html::parse_document("<html><body><p class=\"x\">t</p></body></html>");
            if let Some(node) = doc.get_elements_by_tag_name("p").first().copied() {
                for rule in &sheet.rules {
                    for sel in &rule.selectors.0 {
                        let _ = brows12_css::matcher::matches_selector(&doc, node, sel);
                    }
                }
            }
        }
    }
});
