use std::collections::HashSet;
fn main() {
    let blocker = brows12_privacy::PrivacyBlocker::new();
    let url = "http://127.0.0.1:8901/ads-fixture.html";
    let ps = blocker.page_scripts(url);
    println!("hide_selectors: {}", ps.hide_selectors.len());
    for sel in ps.hide_selectors.iter().take(5) {
        println!("  {sel}");
    }
    println!("exceptions: {}", ps.exceptions.len());
    let sel = blocker.hidden_class_id_selectors(
        vec!["ad-banner".to_string(), "ad-slot".to_string()],
        vec!["net-probe".to_string(), "result".to_string(), "utm-img".to_string()],
        ps.exceptions,
    );
    println!("class/id matched: {}", sel.len());
    for s in sel.iter().take(10) {
        println!("  {s}");
    }
}
