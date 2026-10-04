/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use euclid::default::{Point2D, Rect, Size2D, Transform2D};
use fonts::FontIdentifier;
use kurbo::Shape;
use malloc_size_of::MallocSizeOf;
use paint_api::SerializableImageData;
use pixels::{Snapshot, SnapshotAlphaMode, SnapshotPixelFormat};
use profile_traits::mem::ReportKind;
use servo_base::generic_channel::GenericSharedMemory;
use servo_canvas_traits::canvas::{
    CompositionOptions, CompositionOrBlending, CompositionStyle, FillOrStrokeStyle, FillRule,
    LineOptions, Path, ShadowOptions, TextRun,
};
use servo_config::pref;
use vello_cpu::{RenderSettings, kurbo, peniko};
use webrender_api::{ImageDescriptor, ImageDescriptorFlags};

use crate::backend::{CanvasStoreSizesPerType, Convert, GenericDrawTarget};
use crate::canvas_data::Filter;

thread_local! {
    /// The shared font cache used by all canvases that render on a thread.
    static SHARED_FONT_CACHE: RefCell<HashMap<FontIdentifier, peniko::FontData>> = RefCell::default();
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
enum State {
    /// Scene is drawing. It will be consumed when rendered.
    Drawing,
    /// Scene is already rendered
    /// Before next draw we need to put current rendering
    /// in the background by calling [`VelloCPUDrawTarget::ensure_drawing`].
    Rendered,
}

pub(crate) struct VelloCPUDrawTarget {
    /// Because this is stateful context
    /// caller cannot assume anything about transform, paint, stroke,
    /// so it should provide it's own used by each command
    /// but it can assume paint_transform to be identity
    /// and fill rule to be `peniko::Fill::NonZero`
    ///
    /// This is because paint_transform is rarely set,
    /// so it's cheaper to always reset it after use.
    ctx: vello_cpu::RenderContext,
    resources: vello_cpu::Resources,
    pixmap: vello_cpu::Pixmap,
    clips: Vec<(Path, kurbo::Affine)>,
    state: State,
}

impl VelloCPUDrawTarget {
    fn with_composition(
        &mut self,
        composition_operation: CompositionOrBlending,
        f: impl FnOnce(&mut Self),
    ) {
        // Fast-path for default and most common composition operation
        if composition_operation == CompositionOrBlending::Composition(CompositionStyle::SourceOver)
        {
            f(self);
            return;
        }
        self.ctx.push_blend_layer(composition_operation.convert());
        f(self);
        self.ctx.pop_layer();
    }

    fn ignore_clips(&mut self, f: impl FnOnce(&mut Self)) {
        // pop all clip layers
        for _ in &self.clips {
            self.ctx.pop_layer();
        }
        f(self);
        // push all clip layers back
        for (path, affine) in &self.clips {
            self.ctx.set_transform(*affine);
            self.ctx.push_clip_layer(&path.0);
        }
    }

    fn ensure_drawing(&mut self) {
        match self.state {
            State::Drawing => {},
            State::Rendered => {
                self.ignore_clips(|self_| {
                    self_.ctx.set_transform(kurbo::Affine::IDENTITY);
                    self_.ctx.set_paint(vello_cpu::Image {
                        image: vello_cpu::ImageSource::Pixmap(Arc::new(self_.pixmap.clone())),
                        sampler: peniko::ImageSampler {
                            x_extend: peniko::Extend::Pad,
                            y_extend: peniko::Extend::Pad,
                            quality: peniko::ImageQuality::Low,
                            alpha: 1.0,
                        },
                    });
                    self_.ctx.fill_rect(&kurbo::Rect::from_origin_size(
                        (0., 0.),
                        self_.size().cast(),
                    ));
                });
                self.state = State::Drawing;
            },
        }
    }

    fn pixmap(&mut self) -> &[u8] {
        if self.state == State::Drawing {
            self.ignore_clips(|self_| {
                self_.ctx.flush();
                self_.ctx.render(&mut self_.pixmap, &mut self_.resources);
                self_.ctx.reset();
                self_.state = State::Rendered;
            });
        }

        self.pixmap.data_as_u8_slice()
    }

    fn size(&self) -> Size2D<u32> {
        Size2D::new(self.ctx.width(), self.ctx.height()).cast()
    }

    fn is_viewport_cleared(&mut self, rect: &Rect<f32>, transform: Transform2D<f64>) -> bool {
        let transformed_rect = transform.outer_transformed_rect(&rect.cast());
        if transformed_rect.is_empty() {
            return false;
        }
        let viewport: Rect<f64> = Rect::from_size(self.get_size().cast());
        let Some(clip) = self.clips.iter().try_fold(viewport, |acc, (path, affine)| {
            let mut bez = path.0.clone();
            bez.apply_affine(*affine);
            acc.intersection(&bez.bounding_box().into())
        }) else {
            // clip makes no visible side effects
            return false;
        };
        transformed_rect.cast().contains_rect(&viewport) && // whole viewport is cleared
            clip.contains_rect(&viewport) // viewport is not clipped
    }
}

fn worker_thread_count(size: &Size2D<u16>) -> u16 {
    // TODO: Somewhat arbitrary chosen, should be based on a benchmark,
    // measuring where we start to benefit from multithreading
    const SMALL_CANVAS_SIZE: u32 = 512 * 512;
    // For small sizes single-threaded is better.
    if u32::from(size.width) * u32::from(size.height) < SMALL_CANVAS_SIZE {
        0
    } else {
        // The fallback `3` is the default value from `prefs.rs`
        pref!(thread_pool_canvas_workers).try_into().unwrap_or(3)
    }
}

impl GenericDrawTarget for VelloCPUDrawTarget {
    type SourceSurface = Arc<vello_cpu::Pixmap>;

    fn new(size: Size2D<u32>) -> Self {
        let size = size.cast();
        let settings = RenderSettings {
            num_threads: worker_thread_count(&size),
            ..Default::default()
        };
        let ctx = vello_cpu::RenderContext::new_with(size.width, size.height, settings);
        Self {
            ctx,
            resources: vello_cpu::Resources::new(),
            pixmap: vello_cpu::Pixmap::new(size.width, size.height),
            clips: Vec::new(),
            state: State::Rendered,
        }
    }

    fn canvas_store_sizes(
        &self,
        ops: &mut malloc_size_of::MallocSizeOfOps,
    ) -> Option<Vec<CanvasStoreSizesPerType>> {
        Some(vec![CanvasStoreSizesPerType {
            name: "backing-buffer",
            size: self.pixmap.size_of(ops),
            kind: ReportKind::ExplicitJemallocHeapSize,
        }])
    }

    fn clear_rect(&mut self, rect: &Rect<f32>, transform: Transform2D<f64>) {
        // vello_cpu RenderingContext only ever grows,
        // so we need to use every opportunity to shrink it
        if self.is_viewport_cleared(rect, transform) {
            self.ctx.reset();
            self.clips.clear(); // no clips are affecting rendering
            self.state = State::Drawing;
            return;
        }
        self.ensure_drawing();
        let rect: kurbo::Rect = rect.cast().into();
        let mut clip_path = rect.to_path(0.1);
        clip_path.apply_affine(transform.cast().into());
        let blend_mode = peniko::Compose::Clear;
        self.ctx.push_layer(
            Some(&clip_path.to_path(0.1)),
            Some(blend_mode.into()),
            None,
            None,
            None,
        );
        self.ctx.pop_layer();
    }

    fn copy_surface(
        &mut self,
        surface: Self::SourceSurface,
        source: Rect<i32>,
        destination: Point2D<i32>,
    ) {
        self.ensure_drawing();
        let destination: kurbo::Point = destination.cast::<f64>().into();
        let rect = kurbo::Rect::from_origin_size(destination, source.size.cast());
        self.ctx.set_transform(kurbo::Affine::IDENTITY);
        self.ignore_clips(|self_| {
            // Clipped blending does not work correctly:
            // https://github.com/linebender/vello/issues/1119
            // self_.push_layer(Some(rect.to_path(0.1)), Some(peniko::Compose::Copy.into()), None, None);

            self_.ctx.set_paint(vello_cpu::Image {
                image: vello_cpu::ImageSource::Pixmap(surface),
                sampler: peniko::ImageSampler {
                    x_extend: peniko::Extend::Pad,
                    y_extend: peniko::Extend::Pad,
                    quality: peniko::ImageQuality::Low,
                    alpha: 1.0,
                },
            });
            self_.ctx.fill_rect(&rect);

            // self_.ctx.pop_layer();
        });
    }

    fn create_similar_draw_target(&self, size: &Size2D<i32>) -> Self {
        Self::new(size.cast())
    }

    fn draw_surface(
        &mut self,
        mut surface: Self::SourceSurface,
        dest: Rect<f64>,
        source: Rect<f64>,
        filter: Filter,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        self.ensure_drawing();
        let scale_up = dest.size.width > source.size.width || dest.size.height > source.size.height;
        if composition_options.alpha != 1.0 {
            Arc::get_mut(&mut surface)
                .expect("surface should be owned")
                .multiply_alpha((composition_options.alpha * 255.0) as u8);
        }
        self.with_composition(composition_options.composition_operation, move |self_| {
            self_.ctx.set_transform(transform.cast().into());
            self_.ctx.set_paint(vello_cpu::Image {
                image: vello_cpu::ImageSource::Pixmap(surface),
                sampler: peniko::ImageSampler {
                    x_extend: peniko::Extend::Pad,
                    y_extend: peniko::Extend::Pad,
                    // we should only do bicubic when scaling up
                    quality: if scale_up {
                        filter.convert()
                    } else {
                        peniko::ImageQuality::Low
                    },
                    alpha: 1.0,
                },
            });
            self_.ctx.set_paint_transform(
                kurbo::Affine::translate((dest.origin.x, dest.origin.y)).pre_scale_non_uniform(
                    dest.size.width / source.size.width,
                    dest.size.height / source.size.height,
                ),
            );
            self_.ctx.fill_rect(&dest.into());
            self_.ctx.reset_paint_transform();
        })
    }

    fn draw_surface_with_shadow(
        &mut self,
        surface: Self::SourceSurface,
        dest: &Point2D<f32>,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
    ) {
        self.ensure_drawing();
        let size = Size2D::new(surface.width() as f64, surface.height() as f64);
        let dest = Point2D::new(dest.x as f64, dest.y as f64);
        let shadow_dest = Point2D::new(
            dest.x + shadow_options.offset_x,
            dest.y + shadow_options.offset_y,
        );

        // Per the canvas specification (and matching Chrome and Firefox),
        // `shadowBlur` is twice the standard deviation of the Gaussian blur
        // that is applied to the shape's silhouette.
        let sigma = (shadow_options.blur / 2.0) as f32;
        let shadow_color = shadow_options.color.convert();

        // The source surface contains the shape already composited with the
        // global alpha (it was drawn with the composition options applied).
        // Its alpha channel therefore is the coverage mask of the shadow.
        let shadow_pixmap = tint_blurred_alpha(&surface, sigma, shadow_color);

        let shape_pixmap = Arc::clone(&surface);
        self.with_composition(
            composition_options.composition_operation,
            move |self_| {
                // Step 1: the shadow, drawn first (beneath the shape). The
                // shadow color's alpha is baked into `shadow_pixmap`.
                self_.ctx.set_transform(kurbo::Affine::translate((
                    shadow_dest.x,
                    shadow_dest.y,
                )));
                self_.ctx.set_paint(vello_cpu::Image {
                    image: vello_cpu::ImageSource::Pixmap(Arc::new(shadow_pixmap)),
                    sampler: peniko::ImageSampler {
                        x_extend: peniko::Extend::Pad,
                        y_extend: peniko::Extend::Pad,
                        quality: peniko::ImageQuality::Low,
                        alpha: 1.0,
                    },
                });
                self_.ctx.fill_rect(&kurbo::Rect::new(0.0, 0.0, size.width, size.height));

                // Step 2: the shape itself at its original position.
                self_.ctx
                    .set_transform(kurbo::Affine::translate((dest.x, dest.y)));
                self_.ctx.set_paint(vello_cpu::Image {
                    image: vello_cpu::ImageSource::Pixmap(shape_pixmap),
                    sampler: peniko::ImageSampler {
                        x_extend: peniko::Extend::Pad,
                        y_extend: peniko::Extend::Pad,
                        quality: peniko::ImageQuality::Low,
                        alpha: 1.0,
                    },
                });
                self_.ctx.fill_rect(&kurbo::Rect::new(0.0, 0.0, size.width, size.height));
            },
        );
    }

    fn fill(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        style: FillOrStrokeStyle,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        self.ensure_drawing();
        self.with_composition(composition_options.composition_operation, |self_| {
            self_.ctx.set_transform(transform.cast().into());
            self_.ctx.set_fill_rule(fill_rule.convert());
            self_.ctx.set_paint(paint(style, composition_options.alpha));
            self_.ctx.fill_path(&path.0);
        });
        self.ctx.set_fill_rule(peniko::Fill::NonZero);
    }

    fn fill_text(
        &mut self,
        text_runs: Vec<TextRun>,
        style: FillOrStrokeStyle,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        self.ensure_drawing();
        self.ctx.set_paint(paint(style, composition_options.alpha));
        self.ctx.set_transform(transform.cast().into());
        self.with_composition(composition_options.composition_operation, |self_| {
            for text_run in text_runs.iter() {
                SHARED_FONT_CACHE.with(|font_cache| {
                    let identifier = &text_run.font.identifier;
                    if !font_cache.borrow().contains_key(identifier) {
                        let Some(font_data_and_index) = text_run.font.font_data_and_index() else {
                            return;
                        };
                        let font = font_data_and_index.convert();
                        font_cache.borrow_mut().insert(identifier.clone(), font);
                    }

                    let font_cache = font_cache.borrow();
                    let Some(font) = font_cache.get(identifier) else {
                        return;
                    };
                    self_
                        .ctx
                        .glyph_run(&mut self_.resources, font)
                        .font_size(text_run.pt_size)
                        .fill_glyphs(text_run.glyphs_and_positions.iter().map(
                            |glyph_and_position| vello_cpu::Glyph {
                                id: glyph_and_position.id,
                                x: glyph_and_position.point.x,
                                y: glyph_and_position.point.y,
                            },
                        ));
                });
            }
        })
    }

    fn fill_rect(
        &mut self,
        rect: &Rect<f32>,
        style: FillOrStrokeStyle,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        self.ensure_drawing();
        self.with_composition(composition_options.composition_operation, |self_| {
            self_.ctx.set_transform(transform.cast().into());
            self_.ctx.set_paint(paint(style, composition_options.alpha));
            self_.ctx.fill_rect(&rect.cast().into());
        })
    }

    fn get_size(&self) -> Size2D<i32> {
        self.size().cast()
    }

    fn pop_clip(&mut self) {
        if self.clips.pop().is_some() {
            self.ctx.pop_layer();
        }
    }

    fn push_clip(&mut self, path: &Path, fill_rule: FillRule, transform: Transform2D<f64>) {
        let affine = transform.cast().into();
        self.ctx.set_transform(affine);
        self.ctx.set_fill_rule(fill_rule.convert());
        self.ctx.push_clip_layer(&path.0);
        self.clips.push((path.clone(), affine));
        self.ctx.set_fill_rule(peniko::Fill::NonZero);
    }

    fn push_clip_rect(&mut self, rect: &Rect<i32>) {
        let mut path = Path::new();
        let rect = rect.cast();
        path.rect(
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        );
        self.push_clip(&path, FillRule::Nonzero, Transform2D::identity());
    }

    fn stroke(
        &mut self,
        path: &Path,
        style: FillOrStrokeStyle,
        line_options: LineOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        self.ensure_drawing();
        self.with_composition(composition_options.composition_operation, |self_| {
            self_.ctx.set_transform(transform.cast().into());
            self_.ctx.set_paint(paint(style, composition_options.alpha));
            self_.ctx.set_stroke(line_options.convert());
            self_.ctx.stroke_path(&path.0);
        })
    }

    fn stroke_text(
        &mut self,
        text_runs: Vec<TextRun>,
        style: FillOrStrokeStyle,
        line_options: LineOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        self.ensure_drawing();
        self.ctx.set_paint(paint(style, composition_options.alpha));
        self.ctx.set_stroke(line_options.convert());
        self.ctx.set_transform(transform.cast().into());
        self.with_composition(composition_options.composition_operation, |self_| {
            for text_run in text_runs.iter() {
                SHARED_FONT_CACHE.with(|font_cache| {
                    let identifier = &text_run.font.identifier;
                    if !font_cache.borrow().contains_key(identifier) {
                        let Some(font_data_and_index) = text_run.font.font_data_and_index() else {
                            return;
                        };
                        let font = font_data_and_index.convert();
                        font_cache.borrow_mut().insert(identifier.clone(), font);
                    }

                    let font_cache = font_cache.borrow();
                    let Some(font) = font_cache.get(identifier) else {
                        return;
                    };
                    self_
                        .ctx
                        .glyph_run(&mut self_.resources, font)
                        .font_size(text_run.pt_size)
                        .stroke_glyphs(text_run.glyphs_and_positions.iter().map(
                            |glyph_and_position| vello_cpu::Glyph {
                                id: glyph_and_position.id,
                                x: glyph_and_position.point.x,
                                y: glyph_and_position.point.y,
                            },
                        ));
                });
            }
        })
    }

    fn stroke_rect(
        &mut self,
        rect: &Rect<f32>,
        style: FillOrStrokeStyle,
        line_options: LineOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        self.ensure_drawing();
        self.with_composition(composition_options.composition_operation, |self_| {
            self_.ctx.set_transform(transform.cast().into());
            self_.ctx.set_paint(paint(style, composition_options.alpha));
            self_.ctx.set_stroke(line_options.convert());
            self_.ctx.stroke_rect(&rect.cast().into());
        })
    }

    fn image_descriptor_and_serializable_data(
        &mut self,
    ) -> (ImageDescriptor, SerializableImageData) {
        let image_desc = ImageDescriptor {
            format: webrender_api::ImageFormat::RGBA8,
            size: self.size().cast().cast_unit(),
            stride: None,
            offset: 0,
            flags: ImageDescriptorFlags::empty(),
        };
        let data = SerializableImageData::Raw(GenericSharedMemory::from_bytes(self.pixmap()));
        (image_desc, data)
    }

    fn snapshot(&mut self) -> pixels::Snapshot {
        Snapshot::from_vec(
            self.size().cast(),
            SnapshotPixelFormat::RGBA,
            SnapshotAlphaMode::Transparent {
                premultiplied: true,
            },
            self.pixmap().to_vec(),
        )
    }

    fn surface(&mut self) -> Self::SourceSurface {
        self.pixmap(); // sync pixmap
        Arc::new(vello_cpu::Pixmap::from_parts(
            self.pixmap.clone().take(),
            self.pixmap.width(),
            self.pixmap.height(),
        ))
    }

    fn create_source_surface_from_data(&self, data: Snapshot) -> Option<Self::SourceSurface> {
        Some(snapshot_as_pixmap(data))
    }
}

fn snapshot_as_pixmap(mut snapshot: Snapshot) -> Arc<vello_cpu::Pixmap> {
    let size = snapshot.size().cast();
    snapshot.transform(
        SnapshotAlphaMode::Transparent {
            premultiplied: true,
        },
        SnapshotPixelFormat::RGBA,
    );

    Arc::new(vello_cpu::Pixmap::from_parts(
        bytemuck::cast_vec(snapshot.into()),
        size.width,
        size.height,
    ))
}

impl Convert<vello_cpu::PaintType> for FillOrStrokeStyle {
    fn convert(self) -> vello_cpu::PaintType {
        use servo_canvas_traits::canvas::FillOrStrokeStyle::*;
        match self {
            Color(absolute_color) => vello_cpu::PaintType::Solid(absolute_color.convert()),
            LinearGradient(style) => {
                let start = kurbo::Point::new(style.x0, style.y0);
                let end = kurbo::Point::new(style.x1, style.y1);
                let mut gradient = peniko::Gradient::new_linear(start, end);
                gradient.stops = style.stops.convert();
                vello_cpu::PaintType::Gradient(gradient)
            },
            RadialGradient(style) => {
                let center1 = kurbo::Point::new(style.x0, style.y0);
                let center2 = kurbo::Point::new(style.x1, style.y1);
                let mut gradient = peniko::Gradient::new_two_point_radial(
                    center1,
                    style.r0 as f32,
                    center2,
                    style.r1 as f32,
                );
                gradient.stops = style.stops.convert();
                vello_cpu::PaintType::Gradient(gradient)
            },
            Surface(surface_style) => {
                let pixmap = snapshot_as_pixmap(surface_style.surface_data.to_owned());
                vello_cpu::PaintType::Image(vello_cpu::Image {
                    image: vello_cpu::ImageSource::Pixmap(pixmap),
                    sampler: peniko::ImageSampler {
                        x_extend: if surface_style.repeat_x {
                            peniko::Extend::Repeat
                        } else {
                            peniko::Extend::Pad
                        },
                        y_extend: if surface_style.repeat_y {
                            peniko::Extend::Repeat
                        } else {
                            peniko::Extend::Pad
                        },
                        quality: peniko::ImageQuality::Low,
                        alpha: 1.0,
                    },
                })
            },
        }
    }
}

fn paint(style: FillOrStrokeStyle, alpha: f64) -> vello_cpu::PaintType {
    assert!((0.0..=1.0).contains(&alpha));
    let paint = style.convert();
    if alpha == 1.0 {
        paint
    } else {
        match paint {
            vello_cpu::PaintType::Solid(alpha_color) => {
                vello_cpu::PaintType::Solid(alpha_color.multiply_alpha(alpha as f32))
            },
            vello_cpu::PaintType::Gradient(gradient) => {
                vello_cpu::PaintType::Gradient(gradient.multiply_alpha(alpha as f32))
            },
            vello_cpu::PaintType::Image(mut image) => {
                match &mut image.image {
                    vello_cpu::ImageSource::Pixmap(pixmap) => Arc::get_mut(pixmap)
                        .expect("pixmap should not be shared with anyone at this point")
                        .multiply_alpha((alpha * 255.0) as u8),
                    vello_cpu::ImageSource::OpaqueId { .. } => unimplemented!(),
                };
                vello_cpu::PaintType::Image(image)
            },
        }
    }
}

/// The box blur radius (for a window of `radius * 2 + 1` pixels) whose three
/// passes approximate a Gaussian with the given standard deviation.
fn box_blur_radius_for_sigma(sigma: f32) -> usize {
    // The variance of a single box blur pass with window width `w` is
    // `(w² - 1) / 12`. Three passes therefore give `σ² = (w² - 1) / 4`.
    let width = (4.0 * sigma * sigma + 1.0).sqrt();
    (((width - 1.0) / 2.0).round() as usize).max(1)
}

/// One pass of an edge-clamped sliding-window box blur. `horizontal` selects
/// the axis; the alpha channel is stored one byte per pixel.
fn box_blur_pass(
    src: &[u8],
    dst: &mut [u8],
    width: usize,
    height: usize,
    radius: usize,
    horizontal: bool,
) {
    let window = (radius * 2 + 1) as u32;
    let (outer, inner) = if horizontal { (height, width) } else { (width, height) };
    for outer_index in 0..outer {
        let base = if horizontal { outer_index * width } else { outer_index };
        let sample = |i: usize| -> u8 {
            let clamped = i.min(inner - 1);
            if horizontal {
                src[base + clamped]
            } else {
                src[base + clamped * width]
            }
        };
        // Initialize the window centered at x = 0 with clamped edges.
        let mut sum: u32 = (0..=radius * 2).map(|i| sample(i.min(inner - 1)) as u32).sum();
        for x in 0..inner {
            dst[base + if horizontal { x } else { x * width }] =
                (sum / window) as u8;
            let incoming = sample((x + radius + 1).min(inner - 1)) as u32;
            let outgoing = sample(x.saturating_sub(radius).min(inner - 1)) as u32;
            sum = sum + incoming - outgoing;
        }
    }
}

/// Blur the alpha channel of `pixmap` with a Gaussian approximated by three
/// box blurs, tint it with `color` (whose alpha scales the blurred coverage)
/// and return the result as a new premultiplied RGBA8 pixmap.
fn tint_blurred_alpha(
    pixmap: &vello_cpu::Pixmap,
    sigma: f32,
    color: peniko::Color,
) -> vello_cpu::Pixmap {
    let width = pixmap.width() as usize;
    let height = pixmap.height() as usize;
    let mut alpha: Vec<u8> = {
        let src = pixmap.data_as_u8_slice();
        src.chunks_exact(4).map(|px| px[3]).collect()
    };

    if sigma > 0.0 {
        let radius = box_blur_radius_for_sigma(sigma);
        let mut tmp = vec![0u8; width * height];
        for _ in 0..3 {
            box_blur_pass(&alpha, &mut tmp, width, height, radius, true);
            box_blur_pass(&tmp, &mut alpha, width, height, radius, false);
        }
    }

    let components = color.components;
    let (r, g, b, a) = (
        components[0] as f64,
        components[1] as f64,
        components[2] as f64,
        components[3] as f64,
    );

    let mut out = vello_cpu::Pixmap::new(width as u16, height as u16);
    {
        let dst = out.data_as_u8_slice_mut();
        for (i, coverage) in alpha.iter().enumerate() {
            // Premultiplied output: the color is scaled by the blurred
            // coverage multiplied by the shadow color's alpha.
            let alpha_factor = (*coverage as f64 / 255.0) * a;
            dst[i * 4 + 0] = (r * alpha_factor * 255.0).round().clamp(0.0, 255.0) as u8;
            dst[i * 4 + 1] = (g * alpha_factor * 255.0).round().clamp(0.0, 255.0) as u8;
            dst[i * 4 + 2] = (b * alpha_factor * 255.0).round().clamp(0.0, 255.0) as u8;
            dst[i * 4 + 3] = (alpha_factor * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}
