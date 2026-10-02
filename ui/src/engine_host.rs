//! Engine host thread: owns the `Browser` and every `Tab` (tabs are not
//! `Send`, so all engine calls stay on this thread). The UI thread talks to
//! it through a command channel and receives events + frame snapshots back.
//!
//! This is the integration glue between the shell and the engine: the shell
//! never touches engine types beyond `Frame` and `EngineEvent`.

use crate::model::{UiCmd, UiMsg, START_HTML, START_URL};
use brows12_api::{Browser, EngineEvent, Frame, Tab};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use winit::window::Window;

/// EngineEvent tab id accessor (the enum has no helper).
fn ev_tab(ev: &EngineEvent) -> u64 {
    match ev {
        EngineEvent::NavigationStarted { tab, .. }
        | EngineEvent::NavigationCommitted { tab, .. }
        | EngineEvent::FrameReady { tab, .. }
        | EngineEvent::LoadFinished { tab, .. }
        | EngineEvent::RequestBlocked { tab, .. }
        | EngineEvent::Console { tab, .. }
        | EngineEvent::Suspended { tab }
        | EngineEvent::Resumed { tab } => *tab,
    }
}

pub fn run(
    browser: Browser,
    cmd_rx: Receiver<UiCmd>,
    ui_tx: Sender<UiMsg>,
    wake: std::sync::Arc<Window>,
) {
    let mut events = browser.subscribe();
    let mut tabs: HashMap<u64, std::sync::Arc<Tab>> = HashMap::new();

    // One loop services commands, engine events and frame publishing.
    loop {
        // 1. Drain engine events (Lagged/Disconnected end the drain).
        while let Ok(ev) = events.try_recv() {
            let id = ev_tab(&ev);
            if matches!(ev, EngineEvent::FrameReady { .. }) {
                if let Some(t) = tabs.get(&id) {
                    if let Some(frame) = t.frame() {
                        push_frame(&ui_tx, &wake, id, Some(frame));
                    }
                }
            }
            if ui_tx.send(UiMsg::Engine(ev)).is_err() {
                return;
            }
        }

        // 2. Serve one command (30 ms poll keeps frames/commands responsive).
        match cmd_rx.recv_timeout(Duration::from_millis(30)) {
            Ok(cmd) => {
                if serve(&browser, &mut tabs, &ui_tx, &wake, cmd).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn serve(
    browser: &Browser,
    tabs: &mut HashMap<u64, std::sync::Arc<Tab>>,
    ui_tx: &Sender<UiMsg>,
    wake: &std::sync::Arc<Window>,
    cmd: UiCmd,
) -> Result<(), ()> {
    match cmd {
        UiCmd::NewTab => {
            let tab = browser.new_tab();
            let id = tab.id();
            tabs.insert(id, tab.clone());
            ui_tx.send(UiMsg::TabCreated { id }).map_err(|_| ())?;
            // Fresh tabs render the internal start page (zero network).
            let _ = tab.load_url_from_string(START_HTML, START_URL);
            publish(ui_tx, wake, &tab, id);
        }
        UiCmd::CloseTab { id: Some(id) } => {
            tabs.remove(&id);
            browser.engine().enforce_memory_budget();
        }
        UiCmd::CloseTab { id: None } => {}
        UiCmd::Navigate { tab: id, url } => {
            if let Some(tab) = tabs.get(&id) {
                let result = if url == START_URL {
                    tab.load_url_from_string(START_HTML, &url)
                } else {
                    tab.load_url(&url)
                }
                .map_err(|e| e.to_string());
                ui_tx.send(UiMsg::LoadResult { tab: id, result }).map_err(|_| ())?;
                publish(ui_tx, wake, tab, id);
            }
        }
        UiCmd::Scroll { tab: id, dy } => {
            if let Some(tab) = tabs.get(&id) {
                let _ = tab.set_scroll((tab.scroll_y() + dy).max(0.0));
                publish(ui_tx, wake, tab, id);
            }
        }
    }
    Ok(())
}

/// Publish the newest frame of `tab` to the UI.
fn publish(ui_tx: &Sender<UiMsg>, wake: &std::sync::Arc<Window>, tab: &Tab, id: u64) {
    let frame: Option<Frame> = tab.frame();
    push_frame(ui_tx, wake, id, frame);
}

fn push_frame(ui_tx: &Sender<UiMsg>, wake: &std::sync::Arc<Window>, id: u64, frame: Option<Frame>) {
    if ui_tx.send(UiMsg::Frame { tab: id, frame }).is_ok() {
        wake.request_redraw();
    }
}
