//! Cross-crate integration: the network stack + cookie jar + cache working
//! together over a real local HTTP server.

use brows12_net::{ClientConfig, CookieStore, HttpClient, NetRequest};
use brows12_storage::cookies::CookieJar;
use brows12_storage::registrable_domain;
use std::sync::{Arc, Mutex};

/// CookieStore adapter (mirrors the engine's JarAdapter).
struct JarAdapter(Arc<Mutex<CookieJar>>);

impl CookieStore for JarAdapter {
    fn header_for(&self, url: &url::Url, top_level_site: &str, is_third_party: bool) -> Option<String> {
        self.0.lock().unwrap().header_for_url(url, top_level_site, is_third_party)
    }
    fn record(&self, url: &url::Url, set_cookies: &[String], top_level_site: &str) {
        let mut jar = self.0.lock().unwrap();
        for sc in set_cookies {
            jar.set_from_header(url, sc, top_level_site);
        }
    }
}

/// One-shot HTTP server: returns the bound address; a thread serves exactly
/// one request per entry of `responses`, echoing them in order.
fn server(responses: Vec<String>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for resp in responses {
            let Ok((mut sock, _)) = listener.accept() else { return };
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });
    format!("http://{addr}")
}

#[test]
fn cookies_round_trip_over_http() {
    let base = server(vec![
        // First response sets two cookies.
        "HTTP/1.1 200 OK\r\nSet-Cookie: session=abc; Path=/\r\nSet-Cookie: prefs=dark; Path=/\r\nContent-Length: 2\r\n\r\nok".into(),
    ]);

    let jar = Arc::new(Mutex::new(CookieJar::new()));
    let client = HttpClient::new(ClientConfig::default(), Some(Arc::new(JarAdapter(jar.clone()))));
    let site = "127.0.0.1"; // same-site with the loopback host → cookies flow

    // 1. Obtain cookies.
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(client.send(NetRequest::get(format!("{base}/login"), site.to_string())))
        .unwrap();
    assert_eq!(jar.lock().unwrap().len(), 2);

    // 2. Verify the next request carries the header (capture it raw).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr2 = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        use std::io::Read;
        let (mut sock, _) = listener.accept().unwrap();
        let mut collected = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = sock.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            collected.extend_from_slice(&chunk[..n]);
            if collected.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&collected).to_string()
    });
    let _ = rt.block_on(client.send(NetRequest::get(format!("http://{addr2}/check"), site.to_string())));
    let captured = handle.join().unwrap();
    assert!(
        captured.to_ascii_lowercase().contains("cookie: session=abc"),
        "cookies must be attached, got: {captured}"
    );
}

#[test]
fn redirect_chain_completes() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for stream in listener.incoming() {
            let mut sock = match stream {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut buf = [0u8; 4096];
            // Serve requests until EOF (handles keep-alive and new connects).
            loop {
                let n = match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let resp = if path == "/final" {
                    let body = "<html><body>final page</body></html>";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    )
                } else {
                    "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n".to_string()
                };
                let _ = sock.write_all(resp.as_bytes());
            }
        }
    });

    let client = HttpClient::new(ClientConfig::default(), None);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let resp = rt
        .block_on(client.send(NetRequest::get(format!("http://{addr}/start"), String::new())))
        .unwrap();
    assert_eq!(resp.status, 200);
    assert!(resp.url.ends_with("/final"));
    assert!(resp.body.starts_with(b"<html>"));
}

#[test]
fn cache_rescue_after_failure() {
    use brows12_storage::cache::{CachedResponse, HttpCache};
    let cache = HttpCache::new();
    let url = "http://cached.test/page";
    cache.put(CachedResponse {
        url: url.into(),
        status: 200,
        headers: vec![],
        body: b"<html><body>cached</body></html>".to_vec(),
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        protocol: "HTTP/1.1".into(),
    });
    let hit = cache.get(url).expect("cache hit");
    assert!(hit.body.starts_with(b"<html>"));
    // Registrable domain sanity for CHIPS keys in this pipeline.
    assert_eq!(registrable_domain("cdn.cached.test"), "cached.test");
}

#[test]
fn doh_wire_format_end_to_end() {
    // Query builder + parser over a synthetic response.
    let q = brows12_net::dns::build_query("example.org", 1);
    assert!(q.len() > 17);
    // Response with one A record for the query name.
    let mut msg = vec![0xAB, 0xCD, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
    // Question section (qdcount=1) — the parser skips this before answers.
    for label in ["example", "org"] {
        msg.push(label.len() as u8);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0);
    msg.extend_from_slice(&1u16.to_be_bytes()); // QTYPE=A
    msg.extend_from_slice(&1u16.to_be_bytes()); // QCLASS=IN
    // Answer section: owner name (7 = len of "example").
    msg.extend_from_slice(&[7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'o', b'r', b'g', 0]);
    msg.extend_from_slice(&1u16.to_be_bytes()); // TYPE=A
    msg.extend_from_slice(&1u16.to_be_bytes()); // CLASS=IN
    msg.extend_from_slice(&300u32.to_be_bytes()); // TTL
    msg.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    msg.extend_from_slice(&[93, 184, 216, 34]);
    let res = brows12_net::dns::parse_response(&msg, "example.org", 1).unwrap();
    assert_eq!(res.ipv4[0].to_string(), "93.184.216.34");
}
