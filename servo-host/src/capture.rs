//! Frame capture: paint the WebView, read pixels back, save a PNG.

use std::path::Path;
use std::rc::Rc;

use servo::{DeviceIntRect, RenderingContext, WebView};

/// Renders the current webview state into the rendering context and
/// reads the framebuffer back as an RGBA image.
pub fn capture_webview(webview: &WebView, context: &Rc<dyn RenderingContext>) -> Option<image::RgbaImage> {
    // Paint the latest display list into the context, then read back
    // before presenting (reading works either way; present flushes to
    // the window surface which we do not need headless).
    webview.paint();
    let rect = DeviceIntRect::from_size(context.size2d().to_i32());
    let img = context.read_to_image(rect);
    context.present();
    img
}

/// Saves an RGBA image as PNG at `path`, returning the path on success.
pub fn save_png(img: &image::RgbaImage, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    img.save(path)
        .map_err(|e| std::io::Error::other(format!("png save: {e}")))
}
