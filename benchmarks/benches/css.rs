use brows12_benchmarks::CSS_FIXTURE;
use brows12_css::{Origin, Stylesheet};
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn bench_css(c: &mut Criterion) {
    let mut group = c.benchmark_group("css_parse");
    group.throughput(criterion::Throughput::Bytes(CSS_FIXTURE.len() as u64));
    group.bench_function("parse_rules", |b| {
        b.iter(|| Stylesheet::parse(black_box(CSS_FIXTURE), Origin::Author).unwrap())
    });
    group.finish();
}

criterion_group!(benches, bench_css);
criterion_main!(benches);
