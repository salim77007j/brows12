use brows12_benchmarks::{html_fixture, CSS_FIXTURE};
use brows12_html::parse_document;
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn bench_html(c: &mut Criterion) {
    let small = html_fixture(5);
    let medium = html_fixture(60);
    let large = html_fixture(300);
    let mut group = c.benchmark_group("html_parse");
    group.throughput(criterion::Throughput::Bytes(small.len() as u64));
    group.bench_function("small_5_sections", |b| b.iter(|| parse_document(black_box(&small))));
    group.throughput(criterion::Throughput::Bytes(medium.len() as u64));
    group.bench_function("medium_60_sections", |b| b.iter(|| parse_document(black_box(&medium))));
    group.throughput(criterion::Throughput::Bytes(large.len() as u64));
    group.bench_function("large_300_sections", |b| b.iter(|| parse_document(black_box(&large))));
    group.finish();
}

fn bench_selector_match(c: &mut Criterion) {
    let doc = parse_document(&html_fixture(60));
    let sheet = brows12_css::Stylesheet::parse(CSS_FIXTURE, brows12_css::Origin::Author).unwrap();
    let engine = brows12_css::StyleEngine::with_author_sheets(&[sheet]);
    let ctx = brows12_css::computed::CascadeCtx::default();
    let mut group = c.benchmark_group("cascade");
    group.bench_function("60_sections_full_document", |b| {
        b.iter(|| brows12_css::compute_styles(&doc, &engine, &ctx))
    });
    group.finish();
}

criterion_group!(benches, bench_html, bench_selector_match);
criterion_main!(benches);
