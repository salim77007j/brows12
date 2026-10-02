//! Integration tests for the QuickJS-ng realm: evaluation, console capture,
//! timers, fetch, and DOM mutation.

use brows12_js::{DomHandle, JsEnvironment, JsRuntime, Script};
use brows12_net::{ClientConfig, HttpClient};
use brows12_storage::cookies::CookieJar;
use brows12_storage::{MemoryStore, WebStorage};
use std::sync::{Arc, Mutex};

fn test_env(base_url: &str) -> Arc<JsEnvironment> {
    let tokio_rt =
        Arc::new(tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap());
    let net = Arc::new(HttpClient::new(ClientConfig::default(), None));
    Arc::new(JsEnvironment {
        net,
        tokio: tokio_rt,
        local_storage: WebStorage::local(Arc::new(MemoryStore::new()), base_url),
        cookies: Arc::new(Mutex::new(CookieJar::new())),
        navigator: Default::default(),
        base_url: base_url.to_string(),
        top_level_site: brows12_storage::registrable_domain(
            url::Url::parse(base_url).unwrap().host_str().unwrap_or(""),
        ),
        viewport: (1280, 720),
        console_log: Arc::new(Mutex::new(Vec::new())),
        canvas_store: Arc::new(brows12_js::CanvasStore::new()),
        fonts: Arc::new(Mutex::new(cosmic_text::FontSystem::new())),
    })
}

fn test_runtime() -> (JsRuntime, Arc<JsEnvironment>, DomHandle) {
    let env = test_env("https://example.test/");
    let doc = Arc::new(Mutex::new(brows12_html::parse_document(
        "<html><body><div id=\"app\"></div></body></html>",
    )));
    let dom = DomHandle::new(doc);
    let rt = JsRuntime::new(env.clone(), dom.clone()).unwrap();
    rt.load_glue().unwrap();
    (rt, env, dom)
}

#[test]
fn evaluates_expressions() {
    let (rt, _env, _dom) = test_runtime();
    assert_eq!(rt.eval("1 + 2").unwrap(), "3");
    assert_eq!(rt.eval("'a'.toUpperCase()").unwrap(), "A");
}

#[test]
fn console_output_is_captured() {
    let (rt, env, _dom) = test_runtime();
    rt.eval("console.log('hello', 42); console.warn('careful');").unwrap();
    let log = env.console_log.lock().unwrap();
    assert!(log.iter().any(|l| l.contains("[log] hello 42")));
    assert!(log.iter().any(|l| l.contains("[warn] careful")));
}

#[test]
fn script_errors_are_reported() {
    let (rt, _env, _dom) = test_runtime();
    let err = rt.eval("throw new Error('boom')").unwrap_err();
    assert!(err.to_string().contains("boom"), "got: {err}");
}

#[test]
fn timers_fire_in_event_loop() {
    let (rt, _env, _dom) = test_runtime();
    rt.eval("var order = []; setTimeout(function(){ order.push('t1'); }, 1); order.push('main');")
        .unwrap();
    rt.run_event_loop(std::time::Duration::from_millis(500)).unwrap();
    assert_eq!(rt.eval("order.join(',')").unwrap(), "main,t1");
}

#[test]
fn promises_resolve_via_microtasks() {
    let (rt, _env, _dom) = test_runtime();
    rt.eval("var x = 0; Promise.resolve(21).then(function(v){ x = v * 2; });").unwrap();
    rt.run_event_loop(std::time::Duration::from_millis(100)).unwrap();
    assert_eq!(rt.eval("String(x)").unwrap(), "42");
}

#[test]
fn fetch_against_local_server() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let body = br#"{"answer": 42}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.write_all(body);
        }
    });

    let env = test_env(&format!("http://{addr}/"));
    let doc = Arc::new(Mutex::new(brows12_html::parse_document("<html><body></body></html>")));
    let dom = DomHandle::new(doc);
    let rt = JsRuntime::new(env.clone(), dom.clone()).unwrap();
    rt.load_glue().unwrap();

    rt.eval(&format!(
        "var result = null; var fail = null; fetch('http://{addr}/data').then(function(r) {{ return r.json(); }}).then(function(j) {{ result = j.answer; }}).catch(function(e) {{ fail = String(e); }});"
    ))
    .unwrap();
    rt.run_event_loop(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(rt.eval("String(result)").unwrap(), "42");
    assert_eq!(rt.eval("String(fail)").unwrap(), "null"); // String(null) === "null"
}

#[test]
fn dom_manipulation_from_script() {
    let (rt, _env, dom) = test_runtime();
    rt.eval(
        r#"
        var app = document.getElementById('app');
        var p = document.createElement('p');
        p.setAttribute('class', 'intro');
        p.textContent = 'Injected by QuickJS-ng';
        app.appendChild(p);
        "#,
    )
    .unwrap();
    assert!(dom.take_mutated(), "DOM mutations must flag re-render");
    let html = dom.read(|doc| doc.inner_html(doc.get_element_by_id("app").unwrap())).unwrap();
    assert!(html.contains("Injected by QuickJS-ng"), "got: {html}");
}

#[test]
fn query_selector_works() {
    let (rt, _env, _dom) = test_runtime();
    rt.eval("document.querySelector('#app').setAttribute('data-found', 'yes');").unwrap();
    rt.eval("var count = document.querySelectorAll('div').length;").unwrap();
    assert_eq!(
        rt.eval("document.querySelector('#app').getAttribute('data-found')").unwrap(),
        "yes"
    );
    assert_eq!(rt.eval("String(count)").unwrap(), "1");
}

#[test]
fn local_storage_roundtrip() {
    let (rt, _env, _dom) = test_runtime();
    rt.eval("localStorage.setItem('k', 'v1');").unwrap();
    assert_eq!(rt.eval("localStorage.getItem('k')").unwrap(), "v1");
    rt.eval("localStorage.removeItem('k');").unwrap();
    assert_eq!(rt.eval("String(localStorage.getItem('k'))").unwrap(), "null"); // removed keys read back as null
}

#[test]
fn navigator_is_injected() {
    let (rt, _env, _dom) = test_runtime();
    assert_eq!(rt.eval("navigator.webdriver").unwrap(), "false");
    assert!(rt.eval("navigator.userAgent").unwrap().contains("Brows12"));
}

#[test]
fn location_reflects_base_url() {
    let (rt, _env, _dom) = test_runtime();
    assert_eq!(rt.eval("location.protocol").unwrap(), "https:");
    assert_eq!(rt.eval("location.hostname").unwrap(), "example.test");
}

#[test]
fn execute_reports_script_name() {
    let (rt, _env, _dom) = test_runtime();
    let err = rt
        .execute(&Script::Inline { source: "null.explode()".into(), name: "inline-1".into() })
        .unwrap_err();
    assert!(err.to_string().contains("inline-1"), "got: {err}");
}
