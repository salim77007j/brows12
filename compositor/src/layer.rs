//! Layer model shared by the GPU and CPU compositors.

use tiny_skia::Pixmap;

/// 2D affine transform (translate + rotate + uniform scale) applied at
/// composite time — no re-rasterization of layer contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D {
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
    /// Degrees, counter-clockwise, around the layer origin.
    pub rotate_deg: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Transform2D { tx: 0.0, ty: 0.0, scale: 1.0, rotate_deg: 0.0 }
    }
}

impl Transform2D {
    pub fn is_identity(&self) -> bool {
        self.tx == 0.0 && self.ty == 0.0 && self.scale == 1.0 && self.rotate_deg == 0.0
    }

    /// Row-major 3x3 affine matrix (column vectors): [a b 0; c d 0; e f 1].
    pub fn matrix(&self) -> [f32; 6] {
        let rad = self.rotate_deg.to_radians();
        let (s, c) = (rad.sin(), rad.cos());
        let sc = self.scale;
        [c * sc, s * sc, -s * sc, c * sc, self.tx, self.ty]
    }
}

/// One compositable layer: premultiplied RGBA contents plus placement.
#[derive(Debug, Clone)]
pub struct Layer {
    pub pixmap: Pixmap,
    /// Placement offset in the target surface.
    pub offset: (f32, f32),
    /// Additional scroll offset applied at composite time (GPU scrolls this
    /// way without touching layer contents).
    pub scroll_offset: (f32, f32),
    pub opacity: f32,
    pub transform: Transform2D,
    /// `position: fixed` layers ignore the document scroll offset.
    pub fixed: bool,
}

impl Layer {
    pub fn from_pixmap(pixmap: Pixmap) -> Self {
        Layer {
            pixmap,
            offset: (0.0, 0.0),
            scroll_offset: (0.0, 0.0),
            opacity: 1.0,
            transform: Transform2D::default(),
            fixed: false,
        }
    }

    pub fn with_offset(mut self, x: f32, y: f32) -> Self {
        self.offset = (x, y);
        self
    }

    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }

    pub fn with_scroll(mut self, dx: f32, dy: f32) -> Self {
        self.scroll_offset = (dx, dy);
        self
    }

    pub fn with_transform(mut self, t: Transform2D) -> Self {
        self.transform = t;
        self
    }

    pub fn as_fixed(mut self) -> Self {
        self.fixed = true;
        self
    }

    /// Texture bytes this layer occupies.
    pub fn texture_bytes(&self) -> usize {
        (self.pixmap.width() as usize)
            * (self.pixmap.height() as usize)
            * 4
    }

    /// Total placement including scroll (unless fixed).
    pub fn effective_offset(&self, doc_scroll: (f32, f32)) -> (f32, f32) {
        if self.fixed {
            self.offset
        } else {
            (
                self.offset.0 + doc_scroll.0 + self.scroll_offset.0,
                self.offset.1 + doc_scroll.1 + self.scroll_offset.1,
            )
        }
    }
}
