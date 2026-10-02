use brows12_benchmarks::{html_fixture, CSS_FIXTURE};
use brows12_css::computed::CascadeCtx;
use brows12_css::{compute_styles, StyleEngine};
use brows12_html::parse_document;
use brows12_layout::{compute_layout, TextMeasurer, Viewport};
use criterion::{criterion_group, criterion_main, Criterion};
use std::collections::HashMap;
use std::hint::black_box;
use std::sync::{Arc, Mutex};

fn bench_layout_render(c: &mut Criterion) {
    let doc = parse_document(&html_fixture(60));
    let sheet = brows12_css::Stylesheet::parse(CSS_FIXTURE, brows12_css::Origin::Author).unwrap();
    let engine = StyleEngine::with_author_sheets(&[sheet]);
    let ctx = CascadeCtx::default();
    let styles = compute_styles(&doc, &engine, &ctx);
    let fonts = Arc::new(Mutex::new(cosmic_text_like()));
    let measurer = TextMeasurer::new(fonts.clone());
    let viewport = Viewport { width: 1280.0, height: 720.0 };

    let mut group = c.benchmark_group("layout");
    group.bench_function("60_sections", |b| {
        b.iter(|| compute_layout(&doc, black_box(&styles), viewport, &measurer, &HashMap::new()))
    });
    group.finish();

    // Render
    let layout = compute_layout(&doc, &styles, viewport, &measurer, &HashMap::new());
    let list = brows12_render::build_display_list(
        &doc,
        &styles,
        &layout,
        (1280.0, 720.0),
        &HashMap::new(),
        0.0,
        Default::default(),
    );
    let mut group = c.benchmark_group("render");
    group.bench_function("60_sections_raster_1280x720", |b| {
        let mut rasterizer = brows12_render::Rasterizer::new(fonts.clone());
        b.iter(|| rasterizer.paint(black_box(&list), 1280, 720).unwrap())
    });
    group.finish();
}

fn cosmic_text_like() -> cosmic_text::FontSystem {
    cosmic_text::FontSystem::new()
}

criterion_group!(benches, bench_layout_render);
criterion_main!(benches);
