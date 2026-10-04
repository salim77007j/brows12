//! Rasterization of `clip-path: polygon(..)` (and `path()`/`shape()`) masks.
//!
//! WebRender's clip chain only supports rectangles and rounded rectangles
//! directly. Arbitrary polygons are expressed through the
//! `DisplayItem::ImageMaskClip` item: the polygon is rasterized on the layout
//! thread into a small alpha mask, uploaded to WebRender as an image, and
//! referenced by the clip. This mirrors what Firefox does for basic-shape
//! clips that WebRender cannot represent analytically.
//!
//! The masks are cached across display list builds in a
//! [`PolygonMaskImageCache`] keyed by the (quantized) polygon geometry, so
//! subsequent frames do not re-rasterize or re-upload unchanged polygons.

use std::collections::HashMap;
use std::sync::RwLock;

use webrender_api::units::LayoutPoint;
use webrender_api::{FillRule, ImageKey};

/// Masks larger than this (per dimension) are clamped. Clip-path polygons of
/// this size are pathological; the guard prevents runaway memory use.
const MAX_MASK_DIMENSION: u32 = 8192;

/// Maximum number of cached masks before the cache is reset. Clip-path
/// polygons are rare; a small bound keeps memory usage predictable.
const MAX_CACHE_ENTRIES: usize = 128;

/// The key used to cache polygon masks across display list builds.
///
/// Points are stored relative to the first point and quantized to 1/100 px so
/// that subpixel drift between reflows does not defeat the cache.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PolygonMaskCacheKey {
    quantized_points: Vec<i32>,
    fill_rule: FillRule,
    width: i32,
    height: i32,
}

/// A map from polygon geometry to the WebRender image that holds its mask.
pub(crate) type PolygonMaskImageCache = RwLock<HashMap<PolygonMaskCacheKey, ImageKey>>;

fn quantize(value: f32) -> i32 {
    (value * 100.0).round() as i32
}

pub(crate) fn polygon_mask_cache_key(
    points: &[LayoutPoint],
    fill_rule: FillRule,
    width: u32,
    height: u32,
) -> PolygonMaskCacheKey {
    let origin = points.first().copied().unwrap_or_default();
    let quantized_points = points
        .iter()
        .flat_map(|point| {
            [
                quantize(point.x - origin.x),
                quantize(point.y - origin.y),
            ]
        })
        .collect();
    PolygonMaskCacheKey {
        quantized_points,
        fill_rule,
        width: width as i32,
        height: height as i32,
    }
}

/// Rasterize `points` (already relative to the mask rectangle origin) into a
/// premultiplied BGRA8 image: white, with the polygon coverage in the alpha
/// channel. Coverage is computed from a 2×2 supersampled point-in-polygon test
/// (winding number for `Nonzero`, parity for `Evenodd`), which gives simple
/// but effective edge anti-aliasing.
pub(crate) fn rasterize_polygon_mask(
    points: &[LayoutPoint],
    width: u32,
    height: u32,
    fill_rule: FillRule,
) -> Vec<u8> {
    let width = width.min(MAX_MASK_DIMENSION) as usize;
    let height = height.min(MAX_MASK_DIMENSION) as usize;
    let mut mask = vec![0u8; width * height * 4];
    if points.len() < 3 || width == 0 || height == 0 {
        return mask;
    }

    // Sub-sample offsets inside each pixel (2×2 rotated grid).
    const SAMPLE_OFFSETS: [[f32; 2]; 4] = [
        [0.25, 0.25],
        [0.75, 0.25],
        [0.25, 0.75],
        [0.75, 0.75],
    ];
    const COVERAGE_LUT: [u8; 5] = [0, 64, 128, 191, 255];

    for y in 0..height {
        for x in 0..width {
            let mut inside_samples = 0u8;
            for offset in SAMPLE_OFFSETS {
                let sample_x = x as f32 + offset[0];
                let sample_y = y as f32 + offset[1];
                if polygon_contains(sample_x, sample_y, points, fill_rule) {
                    inside_samples += 1;
                }
            }
            if inside_samples == 0 {
                continue;
            }
            let coverage = COVERAGE_LUT[inside_samples as usize];
            let base = (y * width + x) * 4;
            // Premultiplied white (B, G, R, A byte order for BGRA8).
            mask[base + 0] = coverage;
            mask[base + 1] = coverage;
            mask[base + 2] = coverage;
            mask[base + 3] = coverage;
        }
    }
    mask
}

fn is_left_of_line(
    point_x: f32,
    point_y: f32,
    p0_x: f32,
    p0_y: f32,
    p1_x: f32,
    p1_y: f32,
) -> f32 {
    (p1_x - p0_x) * (point_y - p0_y) - (point_x - p0_x) * (p1_y - p0_y)
}

fn polygon_contains(
    point_x: f32,
    point_y: f32,
    points: &[LayoutPoint],
    fill_rule: FillRule,
) -> bool {
    // A simple winding number computation, matching WebRender's own
    // `polygon_contains_point` used for hit testing.
    let mut winding_number: i32 = 0;
    let count = points.len();
    for i in 0..count {
        let p0 = points[i];
        let p1 = points[(i + 1) % count];
        if p0.y <= point_y {
            if p1.y > point_y &&
                is_left_of_line(point_x, point_y, p0.x, p0.y, p1.x, p1.y) > 0.0
            {
                winding_number += 1;
            }
        } else if p1.y <= point_y &&
            is_left_of_line(point_x, point_y, p0.x, p0.y, p1.x, p1.y) < 0.0
        {
            winding_number -= 1;
        }
    }
    match fill_rule {
        FillRule::Nonzero => winding_number != 0,
        FillRule::Evenodd => winding_number.abs() % 2 == 1,
    }
}
