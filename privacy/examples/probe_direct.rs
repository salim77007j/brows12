use adblock::engine::Engine;
use adblock::request::Request;
fn main() {
    let engine = Engine::new_with_list_text("*$removeparam=utm_source\n");
    let req = Request::new(
        "https://example.com/get?utm_source=brows12test&id=42",
        "example.com",
        "script",
        "GET",
    )
    .unwrap();
    let r = engine.check_network_request(&req);
    println!(
        "direct: filter={:?} important={} rewritten={:?}",
        r.filter.is_some(),
        r.important,
        r.rewritten_url
    );

    // with an explicit type option
    let engine2 = Engine::new_with_list_text("*$removeparam=utm_source,image\n");
    let r2 = engine2.check_network_request(&req);
    println!("image-typed: rewritten={:?}", r2.rewritten_url);

    // hostname anchored
    let engine3 = Engine::new_with_list_text("||example.com^$removeparam=utm_source\n");
    let r3 = engine3.check_network_request(&req);
    println!("hostname: rewritten={:?}", r3.rewritten_url);

    // domains-based
    let engine4 = Engine::new_with_list_text("*$removeparam=utm_source,domain=example.com\n");
    let r4 = engine4.check_network_request(&req);
    println!("domain: rewritten={:?}", r4.rewritten_url);
}
