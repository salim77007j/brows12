//! Probe: print the engine's effective UA and fetch the Wikipedia page the
//! way Tab::load_url does, then report whether RLCONF is present.
#[tokio::main]
async fn main() {
    let engine = brows12_engine::Engine::new(Default::default());
    println!("engine UA: {}", engine.user_agent());
    let client = brows12_net::HttpClient::new(Default::default(), None);
    let mut req = brows12_net::NetRequest::get(
        "https://en.wikipedia.org/wiki/Rust_(programming_language)",
        "https://en.wikipedia.org",
    );
    req.headers.push(("User-Agent".into(), engine.user_agent().to_string()));
    let resp = client.send(req).await.unwrap();
    let html = String::from_utf8_lossy(&resp.body);
    println!("status={} bytes={}", resp.status, resp.body.len());
    println!("has RLCONF: {}", html.contains("RLCONF"));
    println!("has skin-vector-2022: {}", html.contains("skin-vector-2022"));
    // count stylesheet links
    let mut n = 0;
    let mut idx = 0;
    while let Some(p) = html[idx..].find("rel=\"stylesheet\"") {
        n += 1;
        idx += p + 10;
    }
    println!("stylesheet link count: {n}");
}
