use brows12_benchmarks::JS_FIB;
use brows12_js::{DomHandle, JsEnvironment, JsRuntime};
use brows12_net::{ClientConfig, HttpClient};
use brows12_storage::cookies::CookieJar;
use brows12_storage::{MemoryStore, WebStorage};
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use std::sync::{Arc, Mutex};

fn realm() -> JsRuntime {
    let tokio_rt =
        Arc::new(tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap());
    let env = Arc::new(JsEnvironment {
        net: Arc::new(HttpClient::new(ClientConfig::default(), None)),
        tokio: tokio_rt,
        local_storage: WebStorage::local(Arc::new(MemoryStore::new()), "https://bench.test/"),
        cookies: Arc::new(Mutex::new(CookieJar::new())),
        navigator: Default::default(),
        base_url: "https://bench.test/".into(),
        top_level_site: "bench.test".into(),
        viewport: (1280, 720),
        console_log: Arc::new(Mutex::new(Vec::new())),
        canvas_store: Arc::new(brows12_js::CanvasStore::new()),
        webgl_store: Arc::new(brows12_js::WebGlStore::new()),
        kv: Arc::new(brows12_storage::MemoryStore::new()),
        idb_registry: Arc::new(Mutex::new(std::collections::HashMap::new())),
        fonts: Arc::new(Mutex::new(cosmic_text::FontSystem::new())),
    });
    let dom = DomHandle::new(Arc::new(Mutex::new(brows12_html::parse_document(
        "<html><body><div id=\"app\"></div></body></html>",
    ))));
    let rt = JsRuntime::new(env, dom).unwrap();
    rt.load_glue().unwrap();
    rt
}

fn bench_javascript(c: &mut Criterion) {
    let rt = realm();
    let mut group = c.benchmark_group("javascript");

    group.bench_function("eval_fib20", |b| b.iter(|| rt.eval(black_box(JS_FIB)).unwrap()));

    group.bench_function("dom_churn_100_nodes", |b| {
        b.iter(|| {
            rt.eval(
                r#"
                var app = document.getElementById('app');
                app.innerHTML = '';
                for (var i = 0; i < 100; i++) {
                    var d = document.createElement('div');
                    d.className = 'row-' + i;
                    d.textContent = 'item ' + i;
                    app.appendChild(d);
                }
                "#,
            )
            .unwrap()
        })
    });

    group.bench_function("promise_pipeline_100", |b| {
        b.iter(|| {
            rt.eval(
                r#"
                var acc = 0;
                var p = Promise.resolve(1);
                for (var i = 0; i < 100; i++) {
                    p = p.then(function (v) { acc += v; return v + 1; });
                }
                "#,
            )
            .unwrap();
            rt.run_event_loop(std::time::Duration::from_millis(50)).unwrap();
        })
    });

    group.finish();
}

criterion_group!(benches, bench_javascript);
criterion_main!(benches);
