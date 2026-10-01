//! Integration tests for the QuickJS-ng Web API surface: fetch (with
//! bodies), XMLHttpRequest, WebSocket, and Dedicated Workers.

use brows12_html::Document;
use brows12_js::{DomHandle, JsEnvironment, JsRuntime};
use brows12_net::{ClientConfig, HttpClient};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Minimal HTTP/1.1 test server. Routes return fixed responses; `POST`
/// bodies are echoed back prefixed with `post:`.
fn server(routes: Vec<(&'static str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let routes: std::collections::HashMap<String, String> =
        routes.into_iter().map(|(p, c)| (p.to_string(), c)).collect();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => return,
            };
            use std::io::{Read, Write};
            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let method = req.split_whitespace().next().unwrap_or("GET").to_string();
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            let payload = if method == "POST" {
                format!("post:{body}")
            } else {
                routes.get(&path).cloned().unwrap_or_else(|| "not found".into())
            };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                )
                .as_bytes(),
            );
        }
    });
    format!("http://{addr}")
}

fn realm(base_url: String) -> JsRuntime {
    let env = Arc::new(JsEnvironment {
        net: Arc::new(HttpClient::new(ClientConfig::default(), None)),
        tokio: Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        ),
        local_storage: brows12_storage::WebStorage::session("test"),
        cookies: Arc::new(Mutex::new(brows12_storage::cookies::CookieJar::new())),
        navigator: Default::default(),
        base_url,
        top_level_site: "localhost".into(),
        viewport: (800, 600),
        console_log: Arc::new(Mutex::new(Vec::new())),
    });
    let dom = DomHandle::new(Arc::new(Mutex::new(Document::new())));
    let rt = JsRuntime::new(env, dom).expect("realm");
    rt.load_glue().expect("glue");
    rt
}

/// Pump the event loop until `expr` evaluates to true (or timeout).
fn wait_until(rt: &JsRuntime, expr: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let _ = rt.run_event_loop(Duration::from_millis(25));
        if rt.eval(expr).unwrap_or_default() == "true" {
            return true;
        }
    }
    false
}

#[test]
fn fetch_post_sends_body() {
    let base = server(vec![("/", "<h1>hi</h1>".into())]);
    let rt = realm(base.clone());

    rt.eval(&format!(
        "globalThis.__done = false; globalThis.__text = '';
         fetch('{base}/submit', {{ method: 'POST', body: 'hello-world' }})
           .then(function (r) {{ return r.text(); }})
           .then(function (t) {{ globalThis.__text = t; globalThis.__done = true; }});"
    ))
    .unwrap();

    assert!(wait_until(&rt, "globalThis.__done === true", Duration::from_secs(5)));
    assert_eq!(rt.eval("globalThis.__text").unwrap(), "post:hello-world");
}

#[test]
fn fetch_relative_url_resolves_against_location() {
    let base = server(vec![("/api/data", "\"value\"".into())]);
    let rt = realm(base.clone());

    rt.eval(
        "globalThis.__done = false; globalThis.__text = '';
         fetch('/api/data').then(function (r) { return r.json(); }).then(function (j) {
            globalThis.__text = JSON.stringify(j); globalThis.__done = true;
         });",
    )
    .unwrap();

    assert!(wait_until(&rt, "globalThis.__done === true", Duration::from_secs(5)));
    assert_eq!(rt.eval("globalThis.__text").unwrap(), "\"value\"");
}

#[test]
fn xmlhttprequest_round_trip() {
    let base = server(vec![("/data", "xhr-data".into())]);
    let rt = realm(base.clone());

    rt.eval(&format!(
        "globalThis.__done = false; globalThis.__text = ''; globalThis.__status = 0;
         var xhr = new XMLHttpRequest();
         xhr.onreadystatechange = function () {{
           if (xhr.readyState === 4) {{
             globalThis.__done = true;
             globalThis.__text = xhr.responseText;
             globalThis.__status = xhr.status;
           }}
         }};
         xhr.open('GET', '{base}/data');
         xhr.send();"
    ))
    .unwrap();

    assert!(wait_until(&rt, "globalThis.__done === true", Duration::from_secs(5)));
    assert_eq!(rt.eval("globalThis.__text").unwrap(), "xhr-data");
    assert_eq!(rt.eval("globalThis.__status").unwrap(), "200");
}

#[test]
fn websocket_echo_round_trip() {
    // In-thread WebSocket echo server.
    let (addr_tx, addr_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            use futures_util::{SinkExt, StreamExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            addr_tx.send(addr.to_string()).unwrap();
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let ws = match tokio_tungstenite::accept_async(stream).await {
                    Ok(w) => w,
                    Err(_) => continue,
                };
                let (mut tx, mut rx) = ws.split();
                while let Some(Ok(msg)) = rx.next().await {
                    use tokio_tungstenite::tungstenite::Message;
                    match msg {
                        Message::Text(t) => {
                            let reply = format!("echo:{t}");
                            let _ = tx.send(Message::Text(reply.into())).await;
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        });
    });
    let addr = addr_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let rt = realm(format!("http://{addr}"));
    rt.eval(&format!(
        "globalThis.__log = [];
         var ws = new WebSocket('ws://{addr}');
         ws.onopen = function () {{ ws.send('ping'); }};
         ws.onmessage = function (ev) {{ globalThis.__log.push(ev.data); ws.close(); }};"
    ))
    .unwrap();

    assert!(
        wait_until(&rt, "globalThis.__log.length === 1", Duration::from_secs(5)),
        "websocket message never arrived"
    );
    assert_eq!(rt.eval("globalThis.__log[0]").unwrap(), "echo:ping");
    // After close the socket must report the CLOSED ready state.
    let _ = rt.eval("globalThis.__wsState = ws.readyState;");
    assert_eq!(rt.eval("globalThis.__wsState").unwrap(), "3");
}

#[test]
fn worker_message_round_trip() {
    let worker_src = "self.onmessage = function (e) { self.postMessage({ pong: e.data.ping }); };";
    let base = server(vec![("/", "<h1>host</h1>".into()), ("/worker.js", worker_src.to_string())]);
    let rt = realm(base.clone());

    rt.eval(
        "globalThis.__log = []; globalThis.__err = '';
         var w = new Worker('/worker.js');
         w.onerror = function (ev) { globalThis.__err = String(ev.message); };
         w.onmessage = function (ev) { globalThis.__raw = ev.data; globalThis.__log.push(ev.data.pong); };
         w.postMessage({ ping: 42 });",
    )
    .unwrap();

    assert!(
        wait_until(&rt, "globalThis.__log.length === 1", Duration::from_secs(10)),
        "worker message never arrived; worker error: {:?}",
        rt.eval("globalThis.__err").unwrap_or_default()
    );
    assert_eq!(
        rt.eval("globalThis.__log[0]").unwrap(),
        "42",
        "worker payload wrong; raw type={} json={:?}",
        rt.eval("typeof globalThis.__raw").unwrap_or_default(),
        rt.eval("String(globalThis.__raw)").unwrap_or_default()
    );
}
