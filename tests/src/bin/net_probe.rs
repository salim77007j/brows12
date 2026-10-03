//! Probe: fetch the Wikipedia load.php style bundles through brows12's own
//! network stack (the same path the renderer uses) and report status/bytes.
//!
//! Run: cargo run -p brows12-tests --bin net_probe
use std::time::Instant;

#[tokio::main]
async fn main() {
    let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
    let urls = [
        "https://en.wikipedia.org/w/load.php?lang=en&modules=site.styles&only=styles&skin=vector-2022",
        "https://en.wikipedia.org/w/load.php?lang=en&modules=skins.vector.icons%2Cstyles&only=styles&skin=vector-2022",
        "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    ];
    let client = brows12_net::HttpClient::new(Default::default(), None);
    for u in urls {
        let t0 = Instant::now();
        let mut req = brows12_net::NetRequest::get(u, "https://en.wikipedia.org");
        req.headers.push(("User-Agent".into(), ua.into()));
        match client.send(req).await {
            Ok(resp) => println!(
                "{u}\n  status={} bytes={} took={:.2}s ct={:?}\n",
                resp.status,
                resp.body.len(),
                t0.elapsed().as_secs_f32(),
                resp.headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.clone())
            ),
            Err(e) => {
                println!("{u}\n  ERROR {e:?} took={:.2}s\n", t0.elapsed().as_secs_f32())
            }
        }
    }
}
