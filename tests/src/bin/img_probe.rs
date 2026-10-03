//! Probe: fetch a Wikipedia image through brows12's network stack.
#[tokio::main]
async fn main() {
    let client = brows12_net::HttpClient::new(Default::default(), None);
    let t0 = std::time::Instant::now();
    let mut req = brows12_net::NetRequest::get(
        "https://upload.wikimedia.org/wikipedia/en/thumb/3/35/Rust_programming_language_black_logo.svg/100px-Rust_programming_language_black_logo.svg.png",
        "https://en.wikipedia.org",
    );
    req.headers.push(("Accept".into(), "image/avif,image/webp,image/png,image/*,*/*;q=0.8".into()));
    let resp = client.send(req).await;
    match resp {
        Ok(r) => println!(
            "status={} bytes={} took={:.2}s",
            r.status,
            r.body.len(),
            t0.elapsed().as_secs_f32()
        ),
        Err(e) => println!("ERROR {e:?} took={:.2}s", t0.elapsed().as_secs_f32()),
    }
}
