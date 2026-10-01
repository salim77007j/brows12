use brows12_storage::cache::HttpCache;
use brows12_storage::cookies::CookieJar;
use brows12_storage::{MemoryStore, WebStorage};
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use std::sync::Arc;

fn bench_storage(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage");

    group.bench_function("cookie_jar_1000_lookup", |b| {
        let mut jar = CookieJar::new();
        let url = url::Url::parse("https://example.com/").unwrap();
        for i in 0..100 {
            jar.set_from_header(&url, &format!("c{i}=v{i}; Path=/"), "example.com");
        }
        b.iter(|| jar.header_for_url(black_box(&url), "example.com", false))
    });

    group.bench_function("webstorage_put_get", |b| {
        let store = WebStorage::local(Arc::new(MemoryStore::new()), "https://bench.test/");
        b.iter(|| {
            store.set_item("key", "value-0123456789").unwrap();
            black_box(store.get_item("key").unwrap());
        })
    });

    group.bench_function("http_cache_put_get", |b| {
        let cache = HttpCache::new();
        let i = 0u32;
        b.iter(|| {
            let url = format!("https://bench.test/page{}", i);
            cache.put(brows12_storage::cache::CachedResponse {
                url: url.clone(),
                status: 200,
                headers: vec![],
                body: vec![0u8; 4096],
                stored_at: 0,
                protocol: "h2".into(),
            });
            black_box(cache.get(&url));
        })
    });

    group.finish();
}

criterion_group!(benches, bench_storage);
criterion_main!(benches);
