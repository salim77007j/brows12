//! Native functions exposed to the JS realm under the `__brows12` namespace.

use crate::dom_handle::DomHandle;
use crate::environment::JsEnvironment;
use crate::event_loop::{JsTask, TimerQueue};
use crate::worker::WorkerHandle;
use brows12_css::matcher::matches_selector;
use brows12_css::{Origin, Stylesheet};
use brows12_html::{Document, NodeData, NodeId};
use brows12_net::ws::WsTx;
use rquickjs::{Ctx, Function, Object};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Active WebSocket send halves, keyed by the JS-side socket id.
pub type WsSenders = Arc<Mutex<HashMap<u32, WsTx>>>;
/// Live worker handles, keyed by the JS-side worker id.
pub type WorkerRegistry = Arc<Mutex<HashMap<u32, WorkerHandle>>>;

/// How a realm is wired into the rest of the engine.
pub struct RealmLinks {
    /// Completed async work for THIS realm's event loop.
    pub sender: Sender<JsTask>,
    /// Worker realms only: where WorkerMessage/WorkerError flow (the
    /// creating realm's event loop).
    pub upstream: Option<Sender<JsTask>>,
    /// Worker realms only: this worker's id in the creating realm.
    pub worker_id: Option<u32>,
    /// Shared send halves for this realm's WebSocket connections.
    pub ws_senders: WsSenders,
    /// Handles for workers created BY this realm (page realms only).
    pub workers: WorkerRegistry,
}

fn storage_err(e: impl std::fmt::Display) -> rquickjs::Error {
    rquickjs::Error::FromJs { from: "storage", to: "value", message: Some(e.to_string()) }
}

/// Install all native bindings + attach the `__brows12` namespace object.
pub fn install(
    ctx: &Ctx<'_>,
    env: Arc<JsEnvironment>,
    dom: DomHandle,
    links: RealmLinks,
    timers: Arc<Mutex<TimerQueue>>,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let ns = Object::new(ctx.clone())?;
    let is_worker = links.worker_id.is_some();

    // ---- timers -----------------------------------------------------------
    ns.set(
        "timerStart",
        Function::new(ctx.clone(), {
            let timers = timers.clone();
            move |delay_ms: f64, interval: bool| -> u32 {
                let id = timers
                    .lock()
                    .unwrap()
                    .start(Duration::from_millis(delay_ms.max(0.0) as u64), interval);
                id as u32
            }
        })?,
    )?;
    ns.set(
        "timerCancel",
        Function::new(ctx.clone(), {
            let timers = timers.clone();
            move |id: u32| {
                timers.lock().unwrap().cancel(id as u64);
            }
        })?,
    )?;
    ns.set(
        "timerReschedule",
        Function::new(ctx.clone(), {
            let timers = timers.clone();
            move |id: u32, delay_ms: f64| {
                timers
                    .lock()
                    .unwrap()
                    .reschedule(id as u64, Duration::from_millis(delay_ms.max(0.0) as u64));
            }
        })?,
    )?;

    // ---- console ----------------------------------------------------------
    ns.set(
        "consoleLog",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |level: String, msg: String| {
                tracing::debug!(target: "brows12::js", level = %level, "{msg}");
                if let Ok(mut log) = env.console_log.lock() {
                    log.push(format!("[{level}] {msg}"));
                }
            }
        })?,
    )?;

    // ---- fetch ------------------------------------------------------------
    ns.set(
        "fetchStart",
        Function::new(ctx.clone(), {
            let env = env.clone();
            let sender = links.sender.clone();
            move |id: u32, url: String, method: String, headers_json: String, body: String| {
                let net = env.net.clone();
                let top_level_site = env.top_level_site.clone();
                let tx = sender.clone();
                // Policy filter applies to JS-initiated fetches too.
                if net.is_blocked(&url, "fetch") {
                    let _ = tx.send(JsTask::FetchDone {
                        id,
                        err: Some("blocked by content blocker".into()),
                        status: 0,
                        headers: Vec::new(),
                        body: Vec::new(),
                    });
                    return;
                }
                env.tokio.spawn(async move {
                    let mut req = brows12_net::NetRequest::get(url.clone(), top_level_site);
                    if method != "GET" {
                        req.method = method;
                    }
                    if let Ok(hs) = serde_json::from_str::<
                        std::collections::HashMap<String, String>,
                    >(&headers_json)
                    {
                        req.headers = hs.into_iter().collect();
                    }
                    if !body.is_empty() {
                        req.body = Some(body.into_bytes());
                    }
                    let task = match net.send(req).await {
                        Ok(resp) => JsTask::FetchDone {
                            id,
                            err: None,
                            status: resp.status,
                            headers: resp.headers,
                            body: resp.body,
                        },
                        Err(e) => JsTask::FetchDone {
                            id,
                            err: Some(e.to_string()),
                            status: 0,
                            headers: Vec::new(),
                            body: Vec::new(),
                        },
                    };
                    let _ = tx.send(task);
                });
            }
        })?,
    )?;

    crate::platform::install(ctx, &env, &ns)?;
    crate::webgl_bindings::install(ctx, &env, &ns)?;
    crate::wasm::install(ctx, &env, &ns)?;

    // performance.now (ms since realm creation)
    let realm_start = std::time::Instant::now();
    ns.set(
        "perfNow",
        Function::new(ctx.clone(), move || -> f64 {
            realm_start.elapsed().as_secs_f64() * 1000.0
        })?,
    )?;

    // performance.now (ms since realm creation)
    let realm_start = std::time::Instant::now();
    ns.set(
        "perfNow",
        Function::new(ctx.clone(), move || -> f64 {
            realm_start.elapsed().as_secs_f64() * 1000.0
        })?,
    )?;

    // ---- environment data ---------------------------------------------------
    ns.set(
        "locationJson",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move || -> String {
                serde_json::to_string(&env.location_parts()).unwrap_or_else(|_| "{}".into())
            }
        })?,
    )?;
    ns.set(
        "navigatorJson",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move || -> String {
                serde_json::to_string(&env.navigator_parts()).unwrap_or_else(|_| "{}".into())
            }
        })?,
    )?;
    ns.set("randByte", Function::new(ctx.clone(), move || -> u32 { fastrand::u8(..) as u32 })?)?;

    // ---- localStorage -------------------------------------------------------
    ns.set(
        "storageGet",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |key: String| -> rquickjs::Result<Option<String>> {
                env.local_storage.get_item(&key).map_err(storage_err)
            }
        })?,
    )?;
    ns.set(
        "storageSet",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |key: String, value: String| -> rquickjs::Result<()> {
                env.local_storage.set_item(&key, &value).map_err(storage_err)
            }
        })?,
    )?;
    ns.set(
        "storageRemove",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |key: String| -> rquickjs::Result<()> {
                env.local_storage.remove_item(&key).map_err(storage_err)
            }
        })?,
    )?;
    ns.set(
        "storageClear",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move || -> rquickjs::Result<()> { env.local_storage.clear().map_err(storage_err) }
        })?,
    )?;
    ns.set(
        "storageLength",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move || -> u32 { env.local_storage.length().unwrap_or(0) as u32 }
        })?,
    )?;
    ns.set(
        "storageKey",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |index: u32| -> Option<String> {
                env.local_storage.keys().ok()?.into_iter().nth(index as usize)
            }
        })?,
    )?;

    // ---- cookies ------------------------------------------------------------
    ns.set(
        "cookieGet",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move || -> String {
                let url = url::Url::parse(&env.base_url).ok();
                url.and_then(|u| env.cookies.lock().ok().and_then(|j| j.js_cookies(&u)))
                    .unwrap_or_default()
            }
        })?,
    )?;
    ns.set(
        "cookieSet",
        Function::new(ctx.clone(), {
            let env = env.clone();
            move |pair: String| {
                if let Ok(url) = url::Url::parse(&env.base_url) {
                    let top = env.top_level_site.clone();
                    if let Ok(mut jar) = env.cookies.lock() {
                        jar.set_from_header(&url, &pair, &top);
                    }
                }
            }
        })?,
    )?;

    // ---- base64 --------------------------------------------------------------
    ns.set(
        "btoa",
        Function::new(ctx.clone(), move |s: String| -> String {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
        })?,
    )?;
    ns.set(
        "atob",
        Function::new(ctx.clone(), move |s: String| -> rquickjs::Result<String> {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .map_err(storage_err)
        })?,
    )?;

    // ---- WebSockets --------------------------------------------------------
    ns.set(
        "wsOpen",
        Function::new(ctx.clone(), {
            let env = env.clone();
            let sender = links.sender.clone();
            let senders = links.ws_senders.clone();
            move |id: u32, url: String, protocols_json: String| {
                let net = env.net.clone();
                let tx = sender.clone();
                let senders = senders.clone();
                if net.is_blocked(&url, "websocket") {
                    let _ = tx.send(JsTask::WsEvent {
                        id,
                        kind: "error".into(),
                        data: "blocked by content blocker".into(),
                    });
                    let _ =
                        tx.send(JsTask::WsEvent { id, kind: "close".into(), data: String::new() });
                    return;
                }
                let protocols: Vec<String> =
                    serde_json::from_str(&protocols_json).unwrap_or_default();
                env.tokio.spawn(async move {
                    let connect = brows12_net::ws::connect(
                        &url,
                        &protocols,
                        &net.config.user_agent,
                        Duration::from_secs(15),
                    )
                    .await;
                    match connect {
                        Ok(conn) => {
                            senders.lock().unwrap().insert(id, conn.tx.clone());
                            let _ = tx.send(JsTask::WsEvent {
                                id,
                                kind: "open".into(),
                                data: String::new(),
                            });
                            let mut rx = conn.rx;
                            loop {
                                match rx.next().await {
                                    Ok(brows12_net::WsIncoming::Text(t)) => {
                                        let _ = tx.send(JsTask::WsEvent {
                                            id,
                                            kind: "message".into(),
                                            data: t,
                                        });
                                    }
                                    Ok(brows12_net::WsIncoming::Binary(b)) => {
                                        let _ = tx.send(JsTask::WsEvent {
                                            id,
                                            kind: "message".into(),
                                            data: String::from_utf8_lossy(&b).to_string(),
                                        });
                                    }
                                    Ok(brows12_net::WsIncoming::Closed) => {
                                        senders.lock().unwrap().remove(&id);
                                        let _ = tx.send(JsTask::WsEvent {
                                            id,
                                            kind: "close".into(),
                                            data: String::new(),
                                        });
                                        break;
                                    }
                                    Err(e) => {
                                        senders.lock().unwrap().remove(&id);
                                        let _ = tx.send(JsTask::WsEvent {
                                            id,
                                            kind: "error".into(),
                                            data: e.to_string(),
                                        });
                                        let _ = tx.send(JsTask::WsEvent {
                                            id,
                                            kind: "close".into(),
                                            data: String::new(),
                                        });
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(JsTask::WsEvent {
                                id,
                                kind: "error".into(),
                                data: e.to_string(),
                            });
                            let _ = tx.send(JsTask::WsEvent {
                                id,
                                kind: "close".into(),
                                data: String::new(),
                            });
                        }
                    }
                });
            }
        })?,
    )?;
    ns.set(
        "wsSend",
        Function::new(ctx.clone(), {
            let env = env.clone();
            let senders = links.ws_senders.clone();
            move |id: u32, data: String| {
                if let Some(tx) = senders.lock().unwrap().get(&id).cloned() {
                    env.tokio.spawn(async move {
                        let _ = tx.send_text(data).await;
                    });
                }
            }
        })?,
    )?;
    ns.set(
        "wsClose",
        Function::new(ctx.clone(), {
            let env = env.clone();
            let senders = links.ws_senders.clone();
            move |id: u32, _code: u32, _reason: String| {
                if let Some(tx) = senders.lock().unwrap().remove(&id) {
                    env.tokio.spawn(async move {
                        let _ = tx.close().await;
                    });
                }
            }
        })?,
    )?;

    // ---- workers -------------------------------------------------------------
    ns.set(
        "workerNew",
        Function::new(ctx.clone(), {
            let env = env.clone();
            let sender = links.sender.clone();
            let workers = links.workers.clone();
            move |id: u32, url: String| {
                let net = env.net.clone();
                let top_level_site = env.top_level_site.clone();
                let tx = sender.clone();
                let workers = workers.clone();
                let env = env.clone();
                tracing::debug!(target: "brows12::js::worker", "workerNew id={} url={}", id, url);
                if net.is_blocked(&url, "worker") {
                    let _ = tx
                        .send(JsTask::WorkerError { id, err: "blocked by content blocker".into() });
                    return;
                }
                let tokio_rt = env.tokio.clone();
                // Register the mailbox immediately so postMessage calls
                // that race the script fetch are queued, never dropped.
                let (handle, msg_rx) = crate::worker::mailbox();
                workers.lock().unwrap().insert(id, handle.clone());
                let terminate = handle.terminate.clone();
                tokio_rt.spawn(async move {
                    match net.send(brows12_net::NetRequest::get(url.clone(), top_level_site)).await
                    {
                        Ok(resp) if resp.is_success() => {
                            let script = String::from_utf8_lossy(&resp.body).to_string();
                            let mut worker_env = (*env).clone();
                            worker_env.base_url = url;
                            crate::worker::run_in_thread(
                                id,
                                script,
                                Arc::new(worker_env),
                                tx.clone(),
                                msg_rx,
                                terminate,
                            );
                        }
                        Ok(resp) => {
                            workers.lock().unwrap().remove(&id);
                            let _ = tx.send(JsTask::WorkerError {
                                id,
                                err: format!("worker script fetch failed: status {}", resp.status),
                            });
                        }
                        Err(e) => {
                            workers.lock().unwrap().remove(&id);
                            let _ = tx.send(JsTask::WorkerError { id, err: e.to_string() });
                        }
                    }
                });
            }
        })?,
    )?;
    ns.set(
        "workerPost",
        Function::new(ctx.clone(), {
            let workers = links.workers.clone();
            move |id: u32, data: String| {
                if let Some(handle) = workers.lock().unwrap().get(&id) {
                    handle.post(data);
                }
            }
        })?,
    )?;
    ns.set(
        "workerTerminate",
        Function::new(ctx.clone(), {
            let workers = links.workers.clone();
            move |id: u32| {
                if let Some(handle) = workers.lock().unwrap().remove(&id) {
                    handle.terminate();
                }
            }
        })?,
    )?;
    ns.set(
        "workerEmit",
        Function::new(ctx.clone(), {
            let upstream = links.upstream.clone();
            let worker_id = links.worker_id;
            move |data: String| {
                tracing::debug!(target: "brows12::js::worker", "workerEmit {} bytes", data.len());
                if let (Some(upstream), Some(id)) = (&upstream, worker_id) {
                    let _ = upstream.send(JsTask::WorkerMessage { id, data });
                }
            }
        })?,
    )?;

    // ---- realm role ----------------------------------------------------------
    ns.set("isWorker", is_worker)?;

    // ---- DOM (page realms only — workers have no DOM) -------------------------
    if !is_worker {
        install_dom(ctx, &ns, &dom)?;
    }

    globals.set("__brows12", ns)?;
    Ok(())
}

/// DOM native functions. All operate on the shared [`DomHandle`].
fn install_dom<'js>(
    ctx: &Ctx<'js>,
    ns: &Object<'js>,
    dom_handle: &DomHandle,
) -> rquickjs::Result<()> {
    let dom_pool = dom_handle.clone();

    {
        let dom = dom_pool.clone();
        ns.set(
            "nodeType",
            Function::new(ctx.clone(), move |id: i32| -> i32 {
                dom.read(|doc| match doc.node(NodeId(id.max(0) as u32)).data.clone() {
                    NodeData::Document => 9,
                    NodeData::Element { .. } => 1,
                    NodeData::Text(_) => 3,
                    NodeData::Comment(_) => 8,
                    NodeData::Doctype { .. } => 10,
                })
                .unwrap_or(0)
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "parentNode",
            Function::new(ctx.clone(), move |id: i32| -> Option<i32> {
                dom.read(|doc| doc.parent(NodeId(id.max(0) as u32)).map(|p| p.0 as i32)).flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "childCount",
            Function::new(ctx.clone(), move |id: i32| -> i32 {
                dom.read(|doc| doc.children(NodeId(id.max(0) as u32)).len() as i32).unwrap_or(0)
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "childElements",
            Function::new(ctx.clone(), move |id: i32| -> Vec<i32> {
                dom.read(|doc| {
                    doc.child_elements(NodeId(id.max(0) as u32))
                        .into_iter()
                        .map(|n| n.0 as i32)
                        .collect()
                })
                .unwrap_or_default()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "firstElementChild",
            Function::new(ctx.clone(), move |id: i32| -> Option<i32> {
                dom.read(|doc| {
                    doc.first_element_child(NodeId(id.max(0) as u32)).map(|n| n.0 as i32)
                })
                .flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "contains",
            Function::new(ctx.clone(), move |a: i32, b: i32| -> bool {
                dom.read(|doc| {
                    let root = NodeId(a.max(0) as u32);
                    let target = NodeId(b.max(0) as u32);
                    if root == target {
                        return true;
                    }
                    let mut cur = doc.parent(target);
                    while let Some(p) = cur {
                        if p == root {
                            return true;
                        }
                        cur = doc.parent(p);
                    }
                    false
                })
                .unwrap_or(false)
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "tagName",
            Function::new(ctx.clone(), move |id: i32| -> String {
                dom.read(|doc| doc.local_name(NodeId(id.max(0) as u32)).to_string())
                    .unwrap_or_default()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "createElement",
            Function::new(ctx.clone(), move |tag: String| -> i32 {
                dom.mutate(|doc| doc.create_element(&tag).0 as i32).unwrap_or(0)
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "createTextNode",
            Function::new(ctx.clone(), move |text: String| -> i32 {
                dom.mutate(|doc| doc.create_text_node(&text).0 as i32).unwrap_or(0)
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "createComment",
            Function::new(ctx.clone(), move |text: String| -> i32 {
                dom.mutate(|doc| doc.create_comment(&text).0 as i32).unwrap_or(0)
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "getElementById",
            Function::new(ctx.clone(), move |id: String| -> Option<i32> {
                dom.read(|doc| doc.get_element_by_id(&id).map(|n| n.0 as i32)).flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "getElementsByTagName",
            Function::new(ctx.clone(), move |tag: String| -> Vec<i32> {
                dom.read(|doc| {
                    doc.get_elements_by_tag_name(&tag).into_iter().map(|n| n.0 as i32).collect()
                })
                .unwrap_or_default()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "documentElement",
            Function::new(ctx.clone(), move || -> Option<i32> {
                dom.read(|doc| doc.document_element().map(|n| n.0 as i32)).flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "body",
            Function::new(ctx.clone(), move || -> Option<i32> {
                dom.read(|doc| doc.body().map(|n| n.0 as i32)).flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "head",
            Function::new(ctx.clone(), move || -> Option<i32> {
                dom.read(|doc| doc.head().map(|n| n.0 as i32)).flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "title",
            Function::new(ctx.clone(), move || -> String {
                dom.read(|doc| doc.title().unwrap_or_default()).unwrap_or_default()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "setTitle",
            Function::new(ctx.clone(), move |title: String| {
                dom.mutate(|doc| {
                    if let Some(head) = doc.head() {
                        let existing = doc
                            .child_elements(head)
                            .into_iter()
                            .find(|&c| doc.local_name(c) == "title");
                        match existing {
                            Some(t) => doc.set_text_content(t, &title),
                            None => {
                                let t = doc.create_element("title");
                                doc.append_child(head, t);
                                doc.set_text_content(t, &title);
                            }
                        }
                    }
                });
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "getTextContent",
            Function::new(ctx.clone(), move |id: i32| -> String {
                dom.read(|doc| doc.text_content(NodeId(id.max(0) as u32))).unwrap_or_default()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "setTextContent",
            Function::new(ctx.clone(), move |id: i32, text: String| {
                dom.mutate(|doc| doc.set_text_content(NodeId(id.max(0) as u32), &text));
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "getInnerHtml",
            Function::new(ctx.clone(), move |id: i32| -> String {
                dom.read(|doc| doc.inner_html(NodeId(id.max(0) as u32))).unwrap_or_default()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "setInnerHtml",
            Function::new(ctx.clone(), move |id: i32, html: String| {
                dom.mutate(|doc| {
                    let target = NodeId(id.max(0) as u32);
                    let children: Vec<NodeId> = doc.node(target).children.clone();
                    for c in children {
                        doc.remove_child(target, c);
                    }
                    let _ = brows12_html::parse_fragment(doc, target, &html);
                });
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "getAttribute",
            Function::new(ctx.clone(), move |id: i32, name: String| -> Option<String> {
                dom.read(|doc| doc.attr(NodeId(id.max(0) as u32), &name).map(|s| s.to_string()))
                    .flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "setAttribute",
            Function::new(ctx.clone(), move |id: i32, name: String, value: String| {
                dom.mutate(|doc| doc.set_attr(NodeId(id.max(0) as u32), &name, &value));
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "removeAttribute",
            Function::new(ctx.clone(), move |id: i32, name: String| {
                dom.mutate(|doc| doc.remove_attr(NodeId(id.max(0) as u32), &name));
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "appendChild",
            Function::new(ctx.clone(), move |parent: i32, child: i32| {
                dom.mutate(|doc| {
                    doc.append_child(NodeId(parent.max(0) as u32), NodeId(child.max(0) as u32));
                });
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "removeChild",
            Function::new(ctx.clone(), move |parent: i32, child: i32| {
                dom.mutate(|doc| {
                    doc.remove_child(NodeId(parent.max(0) as u32), NodeId(child.max(0) as u32));
                });
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "insertBefore",
            Function::new(ctx.clone(), move |parent: i32, node: i32, reference: i32| {
                dom.mutate(|doc| {
                    doc.insert_before(
                        NodeId(parent.max(0) as u32),
                        NodeId(node.max(0) as u32),
                        NodeId(reference.max(0) as u32),
                    );
                });
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "querySelector",
            Function::new(ctx.clone(), move |root: i32, selector: String| -> Option<i32> {
                dom.read(|doc| {
                    query_selector_impl(doc, root, &selector, true)
                        .into_iter()
                        .next()
                        .map(|n| n.0 as i32)
                })
                .flatten()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "querySelectorAll",
            Function::new(ctx.clone(), move |root: i32, selector: String| -> Vec<i32> {
                dom.read(|doc| {
                    query_selector_impl(doc, root, &selector, false)
                        .into_iter()
                        .map(|n| n.0 as i32)
                        .collect()
                })
                .unwrap_or_default()
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "styleSet",
            Function::new(ctx.clone(), move |id: i32, prop: String, value: String| {
                dom.mutate(|doc| {
                    let node = NodeId(id.max(0) as u32);
                    let existing = doc.attr(node, "style").unwrap_or("").to_string();
                    let updated = update_inline_style(&existing, &prop, &value);
                    doc.set_attr(node, "style", &updated);
                });
            })?,
        )?;
    };
    {
        let dom = dom_pool.clone();
        ns.set(
            "computedStyleJson",
            Function::new(ctx.clone(), move |id: i32| -> String {
                dom.read(|doc| {
                    let node = NodeId(id.max(0) as u32);
                    let mut out = std::collections::BTreeMap::new();
                    if let Some(style_attr) = doc.attr(node, "style") {
                        for decl in style_attr.split(';') {
                            if let Some((k, v)) = decl.split_once(':') {
                                out.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                            }
                        }
                    }
                    serde_json::to_string(&out).unwrap_or_else(|_| "{}".into())
                })
                .unwrap_or_else(|| "{}".into())
            })?,
        )?;
    };

    Ok(())
}

/// Selector matching for querySelector(All): parses the selector through the
/// CSS engine and walks the subtree rooted at `root`.
fn query_selector_impl(doc: &Document, root: i32, selector: &str, first_only: bool) -> Vec<NodeId> {
    let Ok(sheet) = Stylesheet::parse(&format!("{selector} {{}}"), Origin::Author) else {
        return Vec::new();
    };
    let Some(rule) = sheet.rules.first() else {
        return Vec::new();
    };
    let selectors = rule.selectors.clone();
    let start = if root < 0 { doc.root() } else { NodeId(root as u32) };
    let mut out = Vec::new();
    collect_matches(doc, start, &selectors, &mut out, first_only);
    out
}

fn collect_matches(
    doc: &Document,
    node: NodeId,
    selectors: &lightningcss::selector::SelectorList<'static>,
    out: &mut Vec<NodeId>,
    first_only: bool,
) {
    if doc.is_element(node) && selectors.0.iter().any(|sel| matches_selector(doc, node, sel)) {
        out.push(node);
        if first_only {
            return;
        }
    }
    for &c in &doc.node(node).children {
        if first_only && !out.is_empty() {
            return;
        }
        collect_matches(doc, c, selectors, out, first_only);
    }
}

fn update_inline_style(existing: &str, prop: &str, value: &str) -> String {
    let prop = prop.trim().to_ascii_lowercase();
    let mut out: Vec<(String, String)> = Vec::new();
    for decl in existing.split(';') {
        if let Some((k, v)) = decl.split_once(':') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    match out.iter_mut().find(|(k, _)| *k == prop) {
        Some(slot) => slot.1 = value.to_string(),
        None => out.push((prop, value.to_string())),
    }
    out.into_iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("; ")
}
