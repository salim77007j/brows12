//! Tabs and the page load pipeline.

use crate::engine::{EngineEvent, EngineInner};
use crate::EngineError;
use brows12_css::values::Display;
use brows12_css::computed::CascadeCtx;
use brows12_css::{compute_styles, Stylesheet};
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
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Tab {
    pub(crate) fn new(id: u64, engine: Arc<EngineInner>) -> Self {
        Tab { id, engine, state: Mutex::new(TabState::Empty), frame_gen: AtomicU64::new(0) }
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
    pub fn title(&self) -> String {
        match &*self.state.lock().unwrap() {
            TabState::Live(p) => p.title.clone(),
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
        let _ = self.engine.events.send(EngineEvent::NavigationStarted { tab: self.id, url: url.to_string() });

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
        let parsed = url::Url::parse(&target).map_err(|_| brows12_net::NetError::InvalidUrl(target.clone()))?;
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
        let response = match self.engine.tokio.block_on(self.engine.net.send(NetRequest::get(target.clone(), top_site.clone()))) {
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
        let _ = self.engine.events.send(EngineEvent::NavigationCommitted { tab: self.id, url: final_url.clone() });

        // 4. Parse HTML into the arena DOM.
        let html_text = String::from_utf8_lossy(&response.body).to_string();
        let document = Arc::new(Mutex::new(parse_document(&html_text)));
        let dom = DomHandle::new(document.clone());

        // 5. Stylesheets: <link rel=stylesheet> + inline <style>.
        let author_sheets = self.load_stylesheets(&document, &final_url, &top_site)?;
        let engine_sheet = brows12_css::StyleEngine::with_author_sheets(&author_sheets);

        let ctx = CascadeCtx {
            viewport_width: self.engine.config.viewport.width,
            viewport_height: self.engine.config.viewport.height,
            ..CascadeCtx::default()
        };
        let styles = compute_styles(&document.lock().unwrap(), &engine_sheet, &ctx);

        // 6. Images: fetch + decode concurrently.
        let images = self.load_images(&document, &styles, &final_url, &top_site)?;

        // 7. Layout.
        let measurer = brows12_layout::TextMeasurer::new(self.engine.fonts.clone());
        let layout = brows12_layout::compute_layout(
            &document.lock().unwrap(),
            &styles,
            self.engine.config.viewport,
            &measurer,
            &images.iter().map(|(k, v)| (*k, (v.width, v.height))).collect(),
        );

        // 8. Paint generation 1.
        let page = Arc::new(PageInner {
            url: final_url.clone(),
            title: document.lock().unwrap().title().unwrap_or_default(),
            document: document.clone(),
            dom,
            styles: Mutex::new(Some(styles)),
            layout: Mutex::new(Some(layout)),
            frame: RwLock::new(None),
            images: Mutex::new(images),
            last_used: AtomicU64::new(now_secs()),
        });
        {
            let styles = page.styles.lock().unwrap();
            let layout = page.layout.lock().unwrap();
            if let (Some(styles), Some(layout)) = (&*styles, &*layout) {
                self.paint(&page, styles, layout)?;
            }
        }

        page.touch();
        *self.state.lock().unwrap() = TabState::Live(page.clone());
        crate::engine::enforce_budget(&self.engine);
        let _ = self.engine.events.send(EngineEvent::LoadFinished { tab: self.id, title: page.title.clone() });

        // 9. Scripts: build the JS realm and execute in document order.
        self.run_scripts(&page)?;

        // 10. If scripts mutated the DOM: full re-style / re-layout / re-paint.
        if page.dom.take_mutated() {
            let engine_sheet = brows12_css::StyleEngine::with_author_sheets(&author_sheets);
            let styles = compute_styles(&page.document.lock().unwrap(), &engine_sheet, &ctx);
            let layout = brows12_layout::compute_layout(
                &page.document.lock().unwrap(),
                &styles,
                self.engine.config.viewport,
                &measurer,
                &page.images.lock().unwrap().iter().map(|(k, v)| (*k, (v.width, v.height))).collect(),
            );
            self.paint(&page, &styles, &layout)?;
            *page.styles.lock().unwrap() = Some(styles);
            *page.layout.lock().unwrap() = Some(layout);
        }

        Ok(())
    }

    fn paint(
        &self,
        page: &Arc<PageInner>,
        styles: &brows12_css::StyleMap,
        layout: &brows12_layout::LayoutResult,
    ) -> Result<(), EngineError> {
        let images = page.images.lock().unwrap().clone();
        let display_list = build_display_list(
            &page.document.lock().unwrap(),
            styles,
            layout,
            (self.engine.config.viewport.width, self.engine.config.viewport.height),
            &images,
        );
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
                        if doc.attr(node, "rel").map(|r| r.eq_ignore_ascii_case("stylesheet")).unwrap_or(false) {
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
        // Inline <style> blocks first (document order).
        for css in &inline {
            if let Ok(sheet) = Stylesheet::parse(css, brows12_css::Origin::Author) {
                sheets.push(sheet);
            }
        }
        // External sheets, capped to keep hostile pages bounded.
        let base = url::Url::parse(base_url).map_err(|_| brows12_net::NetError::InvalidUrl(base_url.to_string()))?;
        for (i, href) in links.iter().enumerate() {
            if i >= 12 {
                break;
            }
            let resolved = match base.join(href) {
                Ok(u) => u.to_string(),
                Err(_) => continue,
            };
            if let Ok(resp) = self
                .engine
                .tokio
                .block_on(self.engine.net.send(NetRequest::get(resolved.clone(), top_site.to_string())))
            {
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

    fn load_images(
        &self,
        document: &Arc<Mutex<brows12_html::Document>>,
        styles: &brows12_css::StyleMap,
        base_url: &str,
        top_site: &str,
    ) -> Result<HashMap<brows12_html::NodeId, Arc<DecodedImage>>, EngineError> {
        let base = url::Url::parse(base_url).map_err(|_| brows12_net::NetError::InvalidUrl(base_url.to_string()))?;
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
            let Some(src) = document.lock().unwrap().attr(node, "src").map(|s| s.to_string()) else {
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
        // Collect scripts in document order.
        let scripts: Vec<(bool, String)> = {
            let doc = page.document.lock().unwrap();
            let mut out = Vec::new();
            doc.visit_all(|node| {
                if doc.is_element(node) && doc.local_name(node) == "script" {
                    if let Some(src) = doc.attr(node, "src") {
                        out.push((true, src.to_string()));
                    } else {
                        out.push((false, doc.text_content(node)));
                    }
                }
            });
            out
        };

        if scripts.is_empty() {
            return Ok(());
        }

        let base = url::Url::parse(&page.url).map_err(|_| brows12_net::NetError::InvalidUrl(page.url.clone()))?;
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
            viewport: (self.engine.config.viewport.width as u32, self.engine.config.viewport.height as u32),
            console_log: Arc::new(Mutex::new(Vec::new())),
        });

        let runtime = JsRuntime::new(js_env.clone(), page.dom.clone()).map_err(EngineError::from)?;
        runtime.load_glue().map_err(EngineError::from)?;

        let mut executed = 0;
        for (is_external, source) in scripts.iter().take(16) {
            let script = if *is_external {
                let resolved = match base.join(source) {
                    Ok(u) => u.to_string(),
                    Err(_) => continue,
                };
                match self
                    .engine
                    .tokio
                    .block_on(self.engine.net.send(NetRequest::get(resolved, base.host_str().unwrap_or("").to_string())))
                {
                    Ok(resp) if resp.is_success() => Script::External {
                        content: String::from_utf8_lossy(&resp.body).to_string(),
                        name: source.clone(),
                    },
                    _ => continue,
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

    /// Load a document from raw HTML (used for internal pages and tests).
    pub fn load_url_from_string(&self, html: &str, virtual_url: &str) -> Result<(), EngineError> {
        let document = Arc::new(Mutex::new(parse_document(html)));
        let dom = DomHandle::new(document.clone());
        let engine_sheet = brows12_css::StyleEngine::with_author_sheets(&[]);
        let ctx = CascadeCtx {
            viewport_width: self.engine.config.viewport.width,
            viewport_height: self.engine.config.viewport.height,
            ..CascadeCtx::default()
        };
        let styles = compute_styles(&document.lock().unwrap(), &engine_sheet, &ctx);
        let measurer = brows12_layout::TextMeasurer::new(self.engine.fonts.clone());
        let layout = brows12_layout::compute_layout(
            &document.lock().unwrap(),
            &styles,
            self.engine.config.viewport,
            &measurer,
            &HashMap::new(),
        );
        let title = document.lock().unwrap().title().unwrap_or_default();
        let page = Arc::new(PageInner {
            url: virtual_url.to_string(),
            title,
            document,
            dom,
            styles: Mutex::new(Some(styles)),
            layout: Mutex::new(Some(layout)),
            frame: RwLock::new(None),
            images: Mutex::new(HashMap::new()),
            last_used: AtomicU64::new(now_secs()),
        });
        {
            let styles = page.styles.lock().unwrap();
            let layout = page.layout.lock().unwrap();
            if let (Some(styles), Some(layout)) = (&*styles, &*layout) {
                self.paint(&page, styles, layout)?;
            }
        }
        *self.state.lock().unwrap() = TabState::Live(page);
        let _ = self.engine.events.send(EngineEvent::LoadFinished { tab: self.id, title: self.title() });
        Ok(())
    }

    /// Timestamp of last use (for LRU suspension).
    pub(crate) fn last_used_secs(&self) -> u64 {
        match &*self.state.lock().unwrap() {
            TabState::Live(p) => p.last_used.load(Ordering::Relaxed),
            _ => 0,
        }
    }
}
