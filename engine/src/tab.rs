//! Tabs and the page load pipeline.

use crate::engine::{EngineEvent, EngineInner};
use crate::EngineError;
use brows12_css::computed::CascadeCtx;
use brows12_css::values::Display;
use brows12_css::Stylesheet;
use brows12_html::parse_document;
use brows12_js::{DomHandle, JsEnvironment, JsRuntime, Script};
use brows12_net::NetRequest;
use brows12_render::display_list::{build_display_list, DecodedImage};
use brows12_render::raster::{decode_image, Rasterizer};
use brows12_storage::cache::CachedResponse;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// A rendered framebuffer generation.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub generation: u64,
    pub pixmap: Arc<tiny_skia::Pixmap>,
}

impl Frame {
    /// Premultiplied RGBA8 bytes (UI layers can upload directly).
    pub fn rgba_premultiplied(&self) -> &[u8] {
        self.pixmap.data()
    }

    /// Encode the frame as PNG (snapshots, tests).
    pub fn encode_png(&self) -> Result<Vec<u8>, EngineError> {
        Ok(self
            .pixmap
            .clone()
            .encode_png()
            .map_err(|e| brows12_render::RenderError::Pixmap(e.to_string()))?)
    }
}

/// Everything a loaded page owns.
struct PageInner {
    url: String,
    title: String,
    document: Arc<Mutex<brows12_html::Document>>,
    dom: DomHandle,
    styles: Mutex<Option<brows12_css::StyleMap>>,
    layout: Mutex<Option<brows12_layout::LayoutResult>>,
    frame: RwLock<Option<Frame>>,
    images: Mutex<HashMap<brows12_html::NodeId, Arc<DecodedImage>>>,
    last_used: AtomicU64,
    /// Author stylesheets of the current document (for re-cascade).
    author_sheets: Vec<brows12_css::Stylesheet>,
    /// Compositor scroll offset (CSS px).
    scroll_y: Mutex<f32>,
    /// Accumulated animation/transition clock (seconds).
    anim_clock_s: Mutex<f32>,
    /// CSS transition state.
    transitions: Mutex<brows12_css::TransitionEngine>,
    /// Last compositor statistics (backend, frame time, texture bytes).
    compositor_stats: Mutex<Option<brows12_compositor::CompositeStats>>,
}

impl PageInner {
    pub(crate) fn touch(&self) {
        self.last_used.store(now_secs(), Ordering::Relaxed);
    }
}

/// What remains after suspension: url + title + last frame snapshot.
struct SuspendedPage {
    url: String,
    title: String,
    frame: Option<Frame>,
}

enum TabState {
    Empty,
    Live(Arc<PageInner>),
    Suspended(SuspendedPage),
}

/// One browser tab: navigation, rendering and script execution.
pub struct Tab {
    pub(crate) id: u64,
    engine: Arc<EngineInner>,
    state: Mutex<TabState>,
    frame_gen: AtomicU64,
    /// Shared compositor backend (GPU when available, else CPU).
    compositor: std::sync::Mutex<Box<dyn brows12_compositor::Compositor>>,
    /// Canvas2D surfaces created by this tab's page scripts.
    canvas_store: Arc<brows12_js::CanvasStore>,
    webgl_store: Arc<brows12_js::WebGlStore>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn not_live() -> EngineError {
    brows12_net::NetError::InvalidUrl("tab is not live".into()).into()
}

impl Tab {
    pub(crate) fn new(id: u64, engine: Arc<EngineInner>) -> Self {
        Tab {
            id,
            engine,
            state: Mutex::new(TabState::Empty),
            frame_gen: AtomicU64::new(0),
            compositor: std::sync::Mutex::new(brows12_compositor::auto_compositor()),
            canvas_store: Arc::new(brows12_js::CanvasStore::new()),
            webgl_store: Arc::new(brows12_js::WebGlStore::new()),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Current URL (empty when the tab never navigated).
    pub fn url(&self) -> String {
        match &*self.state.lock().unwrap() {
            TabState::Live(p) => p.url.clone(),
            TabState::Suspended(s) => s.url.clone(),
            TabState::Empty => String::new(),
        }
    }

    /// Current document title.
    ///
    /// Read live from the DOM so that titles set by scripts
    /// (`document.title = "..."`) are reflected after `load_*` returns;
    /// the `PageInner::title` snapshot is taken before scripts run.
    pub fn title(&self) -> String {
        match &*self.state.lock().unwrap() {
            TabState::Live(p) => p.document.lock().unwrap().title().unwrap_or_default(),
            TabState::Suspended(s) => s.title.clone(),
            TabState::Empty => String::new(),
        }
    }

    /// Snapshot of the newest framebuffer (if any).
    pub fn frame(&self) -> Option<Frame> {
        match &*self.state.lock().unwrap() {
            TabState::Live(p) => p.frame.read().ok().and_then(|f| f.clone()),
            TabState::Suspended(s) => s.frame.clone(),
            TabState::Empty => None,
        }
    }

    /// Is the page tree currently resident?
    pub fn is_live(&self) -> bool {
        matches!(&*self.state.lock().unwrap(), TabState::Live(_))
    }

    /// Suspend: drop DOM/style/layout trees and the JS realm; keep the
    /// snapshot for instant visual restore.
    pub fn suspend(&self) {
        let mut state = self.state.lock().unwrap();
        if let TabState::Live(page) = &*state {
            *state = TabState::Suspended(SuspendedPage {
                url: page.url.clone(),
                title: page.title.clone(),
                frame: page.frame.read().ok().and_then(|f| f.clone()),
            });
        }
        let _ = self.engine.events.send(EngineEvent::Suspended { tab: self.id });
    }

    /// Resume a suspended tab by reloading (warm cache makes this fast).
    pub fn resume(&self) -> Result<(), EngineError> {
        let url = match &*self.state.lock().unwrap() {
            TabState::Suspended(s) => s.url.clone(),
            _ => return Ok(()),
        };
        let _ = self.engine.events.send(EngineEvent::Resumed { tab: self.id });
        self.load_url(&url).map(|_| ())
    }

    /// Navigate to `url` and run the full load pipeline.
    pub fn load_url(&self, url: &str) -> Result<(), EngineError> {
        let _ = self
            .engine
            .events
            .send(EngineEvent::NavigationStarted { tab: self.id, url: url.to_string() });

        // 1. HTTPS upgrade (localhost/loopback exempt, like real browsers).
        let parsed_probe = url::Url::parse(url).ok();
        let is_local = parsed_probe
            .as_ref()
            .and_then(|u| u.host_str())
            .map(|h| h == "localhost" || h.starts_with("127.") || h == "[::1]")
            .unwrap_or(false);
        let target = if self.engine.config.privacy.https_upgrade && !is_local {
            self.engine.upgrader.upgrade(url)
        } else {
            url.to_string()
        };

        // 2. Privacy check on the document request itself.
        let parsed = url::Url::parse(&target)
            .map_err(|_| brows12_net::NetError::InvalidUrl(target.clone()))?;
        let top_site = brows12_storage::registrable_domain(parsed.host_str().unwrap_or(""));
        let block_kind = brows12_privacy::blocker::RequestKind::Document;
        if let Some(reason) = self.engine.blocker.check(&target, &top_site, block_kind) {
            let _ = self.engine.events.send(EngineEvent::RequestBlocked {
                tab: self.id,
                url: target.clone(),
                reason: format!("{reason:?}"),
            });
            return self.render_local_block_page(&target, &format!("{reason:?}"));
        }

        // 3. Fetch (with cache fallback).
        let response = match self
            .engine
            .tokio
            .block_on(self.engine.net.send(NetRequest::get(target.clone(), top_site.clone())))
        {
            Ok(resp) => {
                self.engine.cache.put(CachedResponse {
                    url: target.clone(),
                    status: resp.status,
                    headers: resp.headers.clone(),
                    body: resp.body.clone(),
                    stored_at: now_secs(),
                    protocol: resp.protocol.clone(),
                });
                resp
            }
            Err(e) => {
                // Cache rescue: offline / failed loads replay the cached copy.
                if let Some(cached) = self.engine.cache.get(&target) {
                    brows12_net::NetResponse {
                        url: target.clone(),
                        status: cached.status,
                        headers: cached.headers,
                        body: cached.body,
                        protocol: cached.protocol,
                    }
                } else {
                    return Err(EngineError::Net(e));
                }
            }
        };

        let final_url = response.url.clone();
        let _ = self
            .engine
            .events
            .send(EngineEvent::NavigationCommitted { tab: self.id, url: final_url.clone() });

        // 4-9. Parse, style, layout, paint, script — the shared HTML pipeline.
        let html_text = String::from_utf8_lossy(&response.body).to_string();
        self.load_html(&html_text, final_url, top_site)
    }

    /// Shared HTML pipeline: parse → stylesheets → page → images → render →
    /// scripts → canvas harvest → re-render. Used by network loads
    /// ([`Tab::load_url`]) and virtual-string loads ([`Tab::load_url_from_string`])
    /// so both execute scripts and honour stylesheets identically.
    fn load_html(
        &self,
        html_text: &str,
        final_url: String,
        top_site: String,
    ) -> Result<(), EngineError> {
        // Navigation resets page-owned surfaces (node ids are per-document).
        self.canvas_store.clear();
        self.webgl_store.clear();

        // Parse HTML into the arena DOM.
        let document = Arc::new(Mutex::new(parse_document(html_text)));
        let dom = DomHandle::new(document.clone());

        // Stylesheets: <link rel=stylesheet> + inline <style>.
        let author_sheets = self.load_stylesheets(&document, &final_url, &top_site)?;

        // Images: fetched during render (display:none filtered via styles).
        let page = Arc::new(PageInner {
            url: final_url.clone(),
            title: document.lock().unwrap().title().unwrap_or_default(),
            document: document.clone(),
            dom,
            styles: Mutex::new(None),
            layout: Mutex::new(None),
            frame: RwLock::new(None),
            images: Mutex::new(HashMap::new()),
            last_used: AtomicU64::new(now_secs()),
            author_sheets,
            scroll_y: Mutex::new(0.0),
            anim_clock_s: Mutex::new(0.0),
            transitions: Mutex::new(brows12_css::TransitionEngine::default()),
            compositor_stats: Mutex::new(None),
        });
        *self.state.lock().unwrap() = TabState::Live(page.clone());
        self.load_images_into(&page)?;
        self.render_page(&page)?;

        page.touch();
        crate::engine::enforce_budget(&self.engine);
        let _ = self
            .engine
            .events
            .send(EngineEvent::LoadFinished { tab: self.id, title: page.title.clone() });

        // Scripts: build the JS realm and execute in document order.
        self.run_scripts(&page)?;

        // Canvas2D surfaces painted by scripts join the image pipeline.
        self.harvest_canvases(&page);
        // WebGL canvases: read back GPU pixels into the same pipeline.
        self.harvest_webgl(&page);

        // If scripts mutated the DOM: full re-style / re-layout / re-paint.
        if page.dom.take_mutated() || !page.images.lock().unwrap().is_empty() {
            self.render_page(&page)?;
        }

        Ok(())
    }

    /// Route Canvas2D surfaces into the image pipeline (canvas participates
    /// in layout + paint like any decoded image).
    fn harvest_canvases(&self, page: &Arc<PageInner>) {
        use brows12_render::display_list::DecodedImage as DI;
        for (node_id, width, height, pixels) in brows12_js::platform::harvest(&self.canvas_store) {
            let node = brows12_html::NodeId(node_id.max(0) as u32);
            let doc = page.document.lock().unwrap();
            let is_canvas = doc.is_element(node) && doc.local_name(node) == "canvas";
            drop(doc);
            if !is_canvas || pixels.is_empty() {
                continue;
            }
            page.images.lock().unwrap().insert(node, Arc::new(DI { width, height, pixels }));
        }
    }

    /// Read back WebGL drawing buffers into the image pipeline.
    fn harvest_webgl(&self, page: &Arc<PageInner>) {
        use brows12_render::display_list::DecodedImage as DI;
        for (node_id, width, height, pixels) in self.webgl_store.harvest() {
            let node = brows12_html::NodeId(node_id.max(0) as u32);
            let doc = page.document.lock().unwrap();
            let is_canvas = doc.is_element(node) && doc.local_name(node) == "canvas";
            drop(doc);
            if !is_canvas || pixels.is_empty() {
                continue;
            }
            page.images.lock().unwrap().insert(node, Arc::new(DI { width, height, pixels }));
        }
    }

    /// Fetch + decode images for the page's `<img>` elements into PageInner.
    fn load_images_into(&self, page: &Arc<PageInner>) -> Result<(), EngineError> {
        let base = url::Url::parse(&page.url)
            .map_err(|_| brows12_net::NetError::InvalidUrl(page.url.clone()))?;
        let top_site = brows12_storage::registrable_domain(base.host_str().unwrap_or(""));

        // First pass: compute styles to skip display:none images.
        let engine_sheet = brows12_css::StyleEngine::with_author_sheets(&page.author_sheets);
        let ctx = CascadeCtx {
            viewport_width: self.engine.config.viewport.width,
            viewport_height: self.engine.config.viewport.height,
            ..CascadeCtx::default()
        };
        let probe_styles =
            brows12_css::compute_styles(&page.document.lock().unwrap(), &engine_sheet, &ctx);

        let nodes: Vec<brows12_html::NodeId> = {
            let doc = page.document.lock().unwrap();
            doc.get_elements_by_tag_name("img")
        };
        let mut out = page.images.lock().unwrap().clone();
        for (i, node) in nodes.into_iter().enumerate() {
            if i >= 24 {
                break;
            }
            if let Some(style) = probe_styles.get(node) {
                if style.display == Display::None {
                    continue;
                }
            }
            let Some(src) = page.document.lock().unwrap().attr(node, "src").map(|s| s.to_string())
            else {
                continue;
            };
            let Ok(resolved) = base.join(&src) else { continue };
            let resolved = resolved.to_string();
            if let Ok(resp) = self.engine.tokio.block_on(
                self.engine.net.send(brows12_net::NetRequest::get(resolved, top_site.clone())),
            ) {
                if resp.is_success() {
                    if let Ok(img) = decode_image(&resp.body) {
                        out.insert(node, Arc::new(img));
                    }
                }
            }
        }
        *page.images.lock().unwrap() = out;
        Ok(())
    }

    fn paint(
        &self,
        page: &Arc<PageInner>,
        styles: &brows12_css::StyleMap,
        layout: &brows12_layout::LayoutResult,
    ) -> Result<(), EngineError> {
        let images = page.images.lock().unwrap().clone();
        let scroll_y = *page.scroll_y.lock().unwrap();
        let display_list = build_display_list(
            &page.document.lock().unwrap(),
            styles,
            layout,
            (self.engine.config.viewport.width, self.engine.config.viewport.height),
            &images,
            scroll_y,
            Default::default(),
        );
        if std::env::var("BROWS_DEBUG").is_ok() {
            for item in &display_list.items {
                match item {
                    brows12_render::display_list::DisplayItem::Rect { rect, color, .. } => {
                        eprintln!(
                            "DL RECT ({:.0},{:.0} {}x{}) {:?}",
                            rect.x, rect.y, rect.width, rect.height, color
                        )
                    }
                    brows12_render::display_list::DisplayItem::Text {
                        rect, text, style, ..
                    } => {
                        eprintln!(
                            "DL TEXT ({:.0},{:.0} {}x{}) fs={} '{:.24}'",
                            rect.x, rect.y, rect.width, rect.height, style.font_size, text
                        )
                    }
                    _ => {}
                }
            }
        }
        let mut rasterizer = Rasterizer::new(self.engine.fonts.clone());
        let (pixmap, _stats) = rasterizer.paint(
            &display_list,
            self.engine.config.viewport.width as u32,
            self.engine.config.viewport.height as u32,
        )?;
        let generation = self.frame_gen.fetch_add(1, Ordering::Relaxed) + 1;
        *page.frame.write().unwrap() = Some(Frame {
            width: pixmap.width(),
            height: pixmap.height(),
            generation,
            pixmap: Arc::new(pixmap),
        });
        let _ = self.engine.events.send(EngineEvent::FrameReady { tab: self.id, generation });
        Ok(())
    }

    /// Full re-render: cascade (+ container pass) -> layout -> paint.
    fn render_page(&self, page: &Arc<PageInner>) -> Result<(), EngineError> {
        // Inline <style> blocks are collected at render time so every path
        // (URL loads and string loads) styles identically.
        let mut sheets = page.author_sheets.clone();
        {
            let doc = page.document.lock().unwrap();
            doc.visit_all(|node| {
                if doc.is_element(node) && doc.local_name(node) == "style" {
                    let css = doc.text_content(node);
                    if let Ok(sheet) =
                        brows12_css::Stylesheet::parse(&css, brows12_css::Origin::Author)
                    {
                        sheets.push(sheet);
                    }
                }
            });
        }
        let engine_sheet = brows12_css::StyleEngine::with_author_sheets(&sheets);
        let base_url = page.url.clone();
        self.load_web_fonts(&engine_sheet, &base_url);
        let ctx = brows12_css::computed::CascadeCtx {
            viewport_width: self.engine.config.viewport.width,
            viewport_height: self.engine.config.viewport.height,
            ..brows12_css::computed::CascadeCtx::default()
        };
        let doc = page.document.clone();
        let measurer = brows12_layout::TextMeasurer::new(self.engine.fonts.clone());
        let image_sizes = || {
            page.images
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (*k, (v.width, v.height)))
                .collect::<HashMap<_, _>>()
        };

        let mut styles = brows12_css::compute_styles(&doc.lock().unwrap(), &engine_sheet, &ctx);
        let mut layout = brows12_layout::compute_layout(
            &doc.lock().unwrap(),
            &styles,
            self.engine.config.viewport,
            &measurer,
            &image_sizes(),
        );

        // Container queries: with sizes from the first layout, re-cascade and
        // re-layout once (converges for the common inline-size cases).
        let has_container_rules = engine_sheet.rules().iter().any(|r| !r.containers.is_empty());
        if has_container_rules {
            let sizes: HashMap<brows12_html::NodeId, f32> = styles
                .styles
                .iter()
                .filter(|(_, s)| s.container_type != brows12_css::values::ContainerType::Normal)
                .filter_map(|(n, _)| layout.rect(*n).map(|r| (*n, r.width)))
                .collect();
            if !sizes.is_empty() {
                styles = brows12_css::compute_styles_with_containers(
                    &doc.lock().unwrap(),
                    &engine_sheet,
                    &ctx,
                    &sizes,
                    &styles,
                );
                layout = brows12_layout::compute_layout(
                    &doc.lock().unwrap(),
                    &styles,
                    self.engine.config.viewport,
                    &measurer,
                    &image_sizes(),
                );
            }
        }

        // CSS transitions: diff against previous styles.
        {
            let mut transitions = page.transitions.lock().unwrap();
            let now = *page.anim_clock_s.lock().unwrap();
            for (node, next) in styles.styles.iter() {
                if let Some(prev) = page.styles.lock().unwrap().as_ref().and_then(|m| m.get(*node))
                {
                    transitions.observe(*node, prev, next, now);
                }
            }
        }

        self.paint(page, &styles, &layout)?;
        *page.styles.lock().unwrap() = Some(styles);
        *page.layout.lock().unwrap() = Some(layout);
        Ok(())
    }

    /// Fetch and register `@font-face` web fonts. Relative URLs resolve
    /// against the page URL; each face's sources are tried in order until
    /// one decodes. Raw TTF/OTF and WOFF1 (zlib table stream) are
    /// decompressed and registered; WOFF2 is not supported by the font
    /// stack (ttf-parser 0.25 has no woff2 tables) and is skipped with a
    /// debug note (pages fall back to system fonts, as intended).
    fn load_web_fonts(&self, engine_sheet: &brows12_css::StyleEngine, base_url: &str) {
        let base = url::Url::parse(base_url).ok();
        let site = base
            .as_ref()
            .and_then(|b| b.host_str().map(brows12_storage::registrable_domain))
            .unwrap_or_default();
        for face in engine_sheet.font_faces.iter().take(6) {
            if face.family.is_empty() {
                continue;
            }
            for src in face.urls.iter() {
                // Resolve relative URLs against the page base.
                let resolved = match &base {
                    Some(b) => match b.join(src) {
                        Ok(u) => u.to_string(),
                        Err(_) => continue,
                    },
                    None => src.clone(),
                };
                let Ok(resp) = self.engine.tokio.block_on(
                    self.engine.net.send(brows12_net::NetRequest::get(resolved, site.clone())),
                ) else {
                    continue;
                };
                if !resp.is_success() || resp.body.is_empty() {
                    continue;
                }
                match classify_font(&resp.body) {
                    FontBytes::Raw => {
                        let aliased = crate::fonts::alias_font_family(&resp.body, &face.family);
                        self.engine.fonts.lock().unwrap().db_mut().load_font_data(aliased);
                        break;
                    }
                    FontBytes::Woff1 => match woff1_decompress(&resp.body) {
                        Ok(raw) => {
                            let aliased = crate::fonts::alias_font_family(&raw, &face.family);
                            self.engine.fonts.lock().unwrap().db_mut().load_font_data(aliased);
                            break;
                        }
                        Err(_) => continue,
                    },
                    FontBytes::Woff2 => {
                        if std::env::var("BROWS_DEBUG").is_ok() {
                            eprintln!(
                                "FONT woff2 not supported (face '{}'), falling back",
                                face.family
                            );
                        }
                        continue;
                    }
                    FontBytes::Unknown => continue,
                }
            }
        }
    }

    fn load_stylesheets(
        &self,
        document: &Arc<Mutex<brows12_html::Document>>,
        base_url: &str,
        top_site: &str,
    ) -> Result<Vec<Stylesheet>, EngineError> {
        let (links, inline) = {
            let doc = document.lock().unwrap();
            let mut links = Vec::new();
            let mut inline = Vec::new();
            doc.visit_all(|node| {
                if !doc.is_element(node) {
                    return;
                }
                match doc.local_name(node) {
                    "link" => {
                        if doc
                            .attr(node, "rel")
                            .map(|r| r.eq_ignore_ascii_case("stylesheet"))
                            .unwrap_or(false)
                        {
                            if let Some(href) = doc.attr(node, "href") {
                                links.push(href.to_string());
                            }
                        }
                    }
                    "style" => inline.push(doc.text_content(node)),
                    _ => {}
                }
            });
            (links, inline)
        };

        let mut sheets = Vec::new();
        // Inline <style> blocks are collected during render_page (shared by
        // the string-load path); here we fetch external sheets only.
        let _ = &inline;
        // External sheets, capped to keep hostile pages bounded.
        let base = url::Url::parse(base_url)
            .map_err(|_| brows12_net::NetError::InvalidUrl(base_url.to_string()))?;
        for (i, href) in links.iter().enumerate() {
            if i >= 12 {
                break;
            }
            let resolved = match base.join(href) {
                Ok(u) => u.to_string(),
                Err(_) => continue,
            };
            if let Ok(resp) = self.engine.tokio.block_on(
                self.engine.net.send(NetRequest::get(resolved.clone(), top_site.to_string())),
            ) {
                if resp.is_success() {
                    let css = String::from_utf8_lossy(&resp.body).to_string();
                    if let Ok(sheet) = Stylesheet::parse(&css, brows12_css::Origin::Author) {
                        sheets.push(sheet);
                    }
                }
            }
        }
        Ok(sheets)
    }

    #[allow(dead_code)]
    fn load_images(
        &self,
        document: &Arc<Mutex<brows12_html::Document>>,
        styles: &brows12_css::StyleMap,
        base_url: &str,
        top_site: &str,
    ) -> Result<HashMap<brows12_html::NodeId, Arc<DecodedImage>>, EngineError> {
        let base = url::Url::parse(base_url)
            .map_err(|_| brows12_net::NetError::InvalidUrl(base_url.to_string()))?;
        let mut out = HashMap::new();
        let nodes: Vec<brows12_html::NodeId> = {
            let doc = document.lock().unwrap();
            doc.get_elements_by_tag_name("img")
        };
        for (i, node) in nodes.into_iter().enumerate() {
            if i >= 24 {
                break;
            }
            // display:none images are not fetched.
            if let Some(style) = styles.get(node) {
                if style.display == Display::None {
                    continue;
                }
            }
            let Some(src) = document.lock().unwrap().attr(node, "src").map(|s| s.to_string())
            else {
                continue;
            };
            let Ok(resolved) = base.join(&src) else { continue };
            let resolved = resolved.to_string();
            if let Ok(resp) = self
                .engine
                .tokio
                .block_on(self.engine.net.send(NetRequest::get(resolved, top_site.to_string())))
            {
                if resp.is_success() {
                    if let Ok(img) = decode_image(&resp.body) {
                        out.insert(node, Arc::new(img));
                    }
                }
            }
        }
        Ok(out)
    }

    fn run_scripts(&self, page: &Arc<PageInner>) -> Result<(), EngineError> {
        // Collect scripts in document order: (is_external, is_module, source).
        let scripts: Vec<(bool, bool, String)> = {
            let doc = page.document.lock().unwrap();
            let mut out = Vec::new();
            doc.visit_all(|node| {
                if doc.is_element(node) && doc.local_name(node) == "script" {
                    let is_module = doc
                        .attr(node, "type")
                        .map(|t| t.eq_ignore_ascii_case("module"))
                        .unwrap_or(false);
                    if let Some(src) = doc.attr(node, "src") {
                        out.push((true, is_module, src.to_string()));
                    } else {
                        out.push((false, is_module, doc.text_content(node)));
                    }
                }
            });
            out
        };

        if scripts.is_empty() {
            return Ok(());
        }

        let base = url::Url::parse(&page.url)
            .map_err(|_| brows12_net::NetError::InvalidUrl(page.url.clone()))?;
        let top_site = brows12_storage::registrable_domain(base.host_str().unwrap_or(""));

        let js_env = Arc::new(JsEnvironment {
            net: self.engine.net.clone(),
            tokio: self.engine.tokio.clone(),
            local_storage: brows12_storage::WebStorage::local(
                self.engine.kv.clone(),
                &format!("{}://{}", base.scheme(), base.host_str().unwrap_or("")),
            ),
            cookies: self.engine.cookies.clone(),
            navigator: if self.engine.config.privacy.strict_fingerprinting {
                brows12_privacy::FingerprintConfig::strict(&self.engine.config.user_agent).navigator
            } else {
                Default::default()
            },
            base_url: page.url.clone(),
            top_level_site: top_site,
            viewport: (
                self.engine.config.viewport.width as u32,
                self.engine.config.viewport.height as u32,
            ),
            console_log: Arc::new(Mutex::new(Vec::new())),
            canvas_store: self.canvas_store.clone(),
            webgl_store: self.webgl_store.clone(),
            kv: self.engine.kv.clone(),
            idb_registry: Arc::new(Mutex::new(std::collections::HashMap::new())),
            fonts: self.engine.fonts.clone(),
        });

        let runtime =
            JsRuntime::new(js_env.clone(), page.dom.clone()).map_err(EngineError::from)?;
        runtime.load_glue().map_err(EngineError::from)?;

        let mut executed = 0;
        for (is_external, is_module, source) in scripts.iter().take(16) {
            let script = if *is_external {
                let resolved = match base.join(source) {
                    Ok(u) => u.to_string(),
                    Err(_) => continue,
                };
                match self.engine.tokio.block_on(
                    self.engine
                        .net
                        .send(NetRequest::get(resolved, base.host_str().unwrap_or("").to_string())),
                ) {
                    Ok(resp) if resp.is_success() => {
                        let content = String::from_utf8_lossy(&resp.body).to_string();
                        if *is_module {
                            Script::ExternalModule { content, name: source.clone() }
                        } else {
                            Script::External { content, name: source.clone() }
                        }
                    }
                    _ => continue,
                }
            } else if *is_module {
                Script::InlineModule {
                    source: source.clone(),
                    name: base
                        .join(&format!("inline-module-{executed}.js"))
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| format!("inline-module-{executed}.js")),
                }
            } else {
                Script::Inline { source: source.clone(), name: format!("inline#{}", executed) }
            };
            match runtime.execute(&script) {
                Ok(()) => executed += 1,
                Err(e) => {
                    let _ = self.engine.events.send(EngineEvent::Console {
                        tab: self.id,
                        message: format!("script error: {e}"),
                    });
                }
            }
        }

        // Let timers/fetches settle, then fire window load events.
        let _ = runtime.run_event_loop(std::time::Duration::from_secs(3));
        let _ = runtime.eval("__brows12FireLoad && __brows12FireLoad();");
        let _ = runtime.run_event_loop(std::time::Duration::from_millis(300));

        // Surface captured console messages as engine events.
        for msg in js_env.console_log.lock().unwrap().drain(..) {
            let _ = self.engine.events.send(EngineEvent::Console { tab: self.id, message: msg });
        }

        Ok(())
    }

    /// Render the built-in blocked page.
    fn render_local_block_page(&self, url: &str, reason: &str) -> Result<(), EngineError> {
        let html = format!(
            "<!DOCTYPE html><html><head><title>Blocked by Brows12</title></head>\
             <body><h1>Blocked</h1><p>This page was blocked by the Brows12 privacy shield.</p>\
             <p style=\"color:#666\">{reason}</p><p style=\"color:#999\">{url}</p></body></html>"
        );
        let escaped_reason = reason.replace('<', "&lt;");
        let html = html.replace("{reason}", &escaped_reason);
        *self.state.lock().unwrap() = TabState::Empty;
        self.load_url_from_string(&html, &format!("brows12://blocked/{}", url))
    }

    /// Load an in-memory HTML document under a virtual URL. Runs the SAME
    /// pipeline as a network load (stylesheets, scripts, canvas harvest) so
    /// fixture pages behave exactly like fetched pages.
    pub fn load_url_from_string(&self, html: &str, virtual_url: &str) -> Result<(), EngineError> {
        let _ = self
            .engine
            .events
            .send(EngineEvent::NavigationStarted { tab: self.id, url: virtual_url.to_string() });
        let _ = self
            .engine
            .events
            .send(EngineEvent::NavigationCommitted { tab: self.id, url: virtual_url.to_string() });
        let top_site = url::Url::parse(virtual_url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();
        self.load_html(html, virtual_url.to_string(), top_site)
    }

    /// Scroll the viewport to `y` (CSS px) and re-composite.
    pub fn set_scroll(&self, y: f32) -> Result<(), EngineError> {
        let page = self.live_page()?;
        let max = page
            .layout
            .lock()
            .unwrap()
            .as_ref()
            .map(|l| (l.content_height - self.engine.config.viewport.height).max(0.0))
            .unwrap_or(0.0);
        *page.scroll_y.lock().unwrap() = y.clamp(0.0, max);
        if let (Some(styles), Some(layout)) =
            (&*page.styles.lock().unwrap(), &*page.layout.lock().unwrap())
        {
            self.paint(&page, styles, layout)?;
        }
        Ok(())
    }

    /// Current scroll offset.
    pub fn scroll_y(&self) -> f32 {
        match self.live_page() {
            Ok(p) => p.scroll_y.lock().map(|g| *g).unwrap_or(0.0),
            Err(_) => 0.0,
        }
    }

    /// Advance the animation/transition clock deterministically: `total_ms`
    /// in `step_ms` ticks, re-styling/re-painting per tick while animations
    /// remain active. Returns the number of painted frames.
    pub fn advance_animation(&self, total_ms: u64, step_ms: u64) -> Result<u32, EngineError> {
        let page = self.live_page()?;
        let engine_sheet = brows12_css::StyleEngine::with_author_sheets(&page.author_sheets);
        let ctx = CascadeCtx {
            viewport_width: self.engine.config.viewport.width,
            viewport_height: self.engine.config.viewport.height,
            ..CascadeCtx::default()
        };
        let base = page.styles.lock().unwrap().clone().unwrap_or_default();
        let measurer = brows12_layout::TextMeasurer::new(self.engine.fonts.clone());
        let step = (step_ms.max(1) as f32) / 1000.0;
        let total = total_ms as f32 / 1000.0;
        let mut t = 0.0f32;
        let mut frames = 0u32;
        loop {
            let mut styles = base.clone();
            let mut any_active = false;
            for (node, s) in styles.styles.iter_mut() {
                let spec = s.animation.clone();
                if let Some(spec) = spec {
                    if let Some(kf) = engine_sheet.keyframes.get(&spec.name) {
                        // Snapshot the pre-animation style as the underlying
                        // value reference (borrow checker: separate from &mut s).
                        let underlying = s.clone();
                        let base_ref: &brows12_css::ComputedStyle =
                            base.styles.get(node).unwrap_or(&underlying);
                        let status =
                            brows12_css::apply_keyframes(s, base_ref, kf, &spec, t, &ctx, None);
                        if status == brows12_css::AnimationStatus::Active {
                            any_active = true;
                        }
                    }
                }
                page.transitions.lock().unwrap().apply(*node, s, &ctx, None, t);
            }
            let layout = brows12_layout::compute_layout(
                &page.document.lock().unwrap(),
                &styles,
                self.engine.config.viewport,
                &measurer,
                &page
                    .images
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (*k, (v.width, v.height)))
                    .collect(),
            );
            self.paint(&page, &styles, &layout)?;
            frames += 1;
            *page.anim_clock_s.lock().unwrap() = t;
            t += step;
            if t > total || !any_active {
                break;
            }
        }
        // Restore the settled styles (fill-mode handling included).
        Ok(frames)
    }

    /// Full re-render from the current DOM (scripts, style edits).
    pub fn refresh(&self) -> Result<(), EngineError> {
        let page = self.live_page()?;
        self.render_page(&page)
    }

    /// Composite through the compositor: content layer + fixed overlay
    /// layer(s), scroll applied by the backend (GPU moves textures; CPU
    /// blits). Produces the same visual result as `paint` but exercises the
    /// GPU path and reports backend/frame statistics.
    pub fn composite_frame(&self) -> Result<Frame, EngineError> {
        use brows12_compositor::Layer;
        use brows12_render::display_list::ListScope;

        let page = self.live_page()?;
        let styles = page.styles.lock().unwrap().clone().ok_or_else(not_live)?;
        let layout = page.layout.lock().unwrap().clone().ok_or_else(not_live)?;
        let scroll_y = *page.scroll_y.lock().unwrap();
        let images = page.images.lock().unwrap().clone();
        let vw = self.engine.config.viewport.width as u32;
        let vh = self.engine.config.viewport.height as u32;
        let doc = page.document.lock().unwrap();

        // Content layer: whole document raster (capped), scroll not baked in.
        let content_h = (layout.content_height as u32).clamp(vh, 16384);
        let content_list = build_display_list(
            &doc,
            &styles,
            &layout,
            (content_h as f32, content_h as f32),
            &images,
            0.0,
            ListScope::Content,
        );
        let mut rasterizer = Rasterizer::new(self.engine.fonts.clone());
        let (content_pm, _) = rasterizer.paint(&content_list, vw, content_h.max(vh))?;

        let mut layers = vec![Layer::from_pixmap(content_pm).with_scroll(0.0, -scroll_y)];

        // Fixed overlay layer: viewport-aligned, ignores scroll.
        let fixed_list = build_display_list(
            &doc,
            &styles,
            &layout,
            (self.engine.config.viewport.width, self.engine.config.viewport.height),
            &images,
            0.0,
            ListScope::Fixed,
        );
        if !fixed_list.items.is_empty() {
            let (fixed_pm, _) = rasterizer.paint(&fixed_list, vw, vh)?;
            layers.push(Layer::from_pixmap(fixed_pm).as_fixed());
        }
        drop(doc);

        let compositor = self.compositor.lock().unwrap();
        let out = compositor
            .composite(vw, vh, &layers)
            .map_err(|e| EngineError::Compositor(e.to_string()))?;
        let stats = out.stats.clone();
        let pixmap = out.to_pixmap().ok_or_else(not_live)?;
        *page.compositor_stats.lock().unwrap() = Some(stats);
        let generation = self.frame_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let frame = Frame {
            width: pixmap.width(),
            height: pixmap.height(),
            generation,
            pixmap: Arc::new(pixmap),
        };
        *page.frame.write().unwrap() = Some(frame.clone());
        let _ = self.engine.events.send(EngineEvent::FrameReady { tab: self.id, generation });
        Ok(frame)
    }

    /// Backend + timing stats of the most recent `composite_frame`.
    pub fn compositor_stats(&self) -> Option<brows12_compositor::CompositeStats> {
        self.live_page().ok().and_then(|p| p.compositor_stats.lock().ok().and_then(|g| g.clone()))
    }

    fn live_page(&self) -> Result<Arc<PageInner>, EngineError> {
        match &*self.state.lock().unwrap() {
            TabState::Live(p) => Ok(p.clone()),
            _ => Err(brows12_net::NetError::InvalidUrl("tab is not live".into()).into()),
        }
    }

    /// Timestamp of last use (for LRU suspension).
    pub(crate) fn last_used_secs(&self) -> u64 {
        match &*self.state.lock().unwrap() {
            TabState::Live(p) => p.last_used.load(Ordering::Relaxed),
            _ => 0,
        }
    }
}

/// Sniff font file magic bytes.
enum FontBytes {
    Raw,
    Woff1,
    Woff2,
    Unknown,
}

fn classify_font(b: &[u8]) -> FontBytes {
    if b.len() < 4 {
        return FontBytes::Unknown;
    }
    match &b[0..4] {
        // sfnt versions: 0x00010000 (ttc), 'true', 'OTTO' (cff), 'ttcf'
        [0x00, 0x01, 0x00, 0x00] => FontBytes::Raw,
        [b't', b'r', b'u', b'e'] | [b'O', b'T', b'T', b'O'] | [b't', b't', b'c', b'f'] => {
            FontBytes::Raw
        }
        [b'w', b'O', b'F', b'F'] => FontBytes::Woff1,
        [b'w', b'O', b'F', b'2'] => FontBytes::Woff2,
        _ => FontBytes::Unknown,
    }
}

/// Decompress a WOFF1 container into a raw sfnt font (WOFF2's transformed
/// glyf stream is out of scope; WOFF1 tables are plain zlib streams).
/// Format: <WOFF header><table directory (compressed)>[table data] —
/// rebuilds the original table order with 4-byte alignment.
fn woff1_decompress(data: &[u8]) -> Result<Vec<u8>, ()> {
    use std::io::Read;
    if data.len() < 44 || &data[0..4] != b"wOFF" {
        return Err(());
    }
    let be32 = |o: usize| -> u32 {
        u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
    };
    let flavor = be32(4);
    let num_tables = be32(12) as usize;
    if num_tables == 0 || num_tables > 512 {
        return Err(());
    }
    // Rebuild the table directory.
    struct Entry {
        tag: [u8; 4],
        orig_checksum: u32,
        orig_len: u32,
        data: Vec<u8>,
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(num_tables);
    let mut off = 44usize;
    for _ in 0..num_tables {
        if off + 20 > data.len() {
            return Err(());
        }
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[off..off + 4]);
        let offset = be32(off + 4) as usize;
        let comp_len = be32(off + 8) as usize;
        let orig_len = be32(off + 12) as usize;
        let orig_checksum = be32(off + 16);
        if offset.saturating_add(comp_len) > data.len() {
            return Err(());
        }
        let raw = &data[offset..offset + comp_len];
        let table = if comp_len < orig_len {
            let mut out = Vec::with_capacity(orig_len);
            let mut dec = flate2::read::ZlibDecoder::new(raw);
            dec.read_to_end(&mut out).map_err(|_| ())?;
            if out.len() != orig_len {
                return Err(());
            }
            out
        } else {
            raw.to_vec()
        };
        entries.push(Entry { tag, orig_checksum, orig_len: orig_len as u32, data: table });
        off += 20;
    }

    // Assemble sfnt: header (12) + directory (16 * n) + tables (4-aligned).
    let dir_size = 12 + 16 * num_tables;
    let total: usize = entries.iter().map(|e| (e.data.len() + 3) & !3).sum::<usize>() + dir_size;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num_tables as u16).to_be_bytes());
    // searchRange/entrySelector/rangeShift (per spec, from num_tables).
    let mut entry_selector = 0u16;
    while (1u16 << (entry_selector + 1)) <= num_tables as u16 {
        entry_selector += 1;
    }
    let search_range = (1u16 << entry_selector) * 16;
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&((num_tables as u16) * 16 - search_range).to_be_bytes());

    // sfnt directory entries must be sorted by tag.
    entries.sort_by(|a, b| a.tag.cmp(&b.tag));
    let mut offset_cursor = dir_size as u32;
    for e in &entries {
        out.extend_from_slice(&e.tag);
        out.extend_from_slice(&e.orig_checksum.to_be_bytes());
        out.extend_from_slice(&offset_cursor.to_be_bytes());
        out.extend_from_slice(&e.orig_len.to_be_bytes());
        offset_cursor += (e.data.len() as u32 + 3) & !3;
    }
    for e in &entries {
        out.extend_from_slice(&e.data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Ok(out)
}
