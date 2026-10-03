//! Event-loop waker bridging Servo's internal IPC to the winit loop.

use std::sync::Arc;

use servo::EventLoopWaker;
use winit::event_loop::EventLoopProxy;

#[derive(Debug)]
pub struct HostWakerEvent;

#[derive(Clone)]
pub struct ProxyWaker(pub Arc<EventLoopProxy<HostWakerEvent>>);

impl ProxyWaker {
    pub fn new(proxy: EventLoopProxy<HostWakerEvent>) -> Self {
        Self(Arc::new(proxy))
    }
}

impl EventLoopWaker for ProxyWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        // Best effort: if the loop is gone we are shutting down anyway.
        let _ = self.0.send_event(HostWakerEvent);
    }
}

/// A waker for non-winit (software rendering) loops: a condvar the spin
/// loop waits on with a timeout.
#[derive(Clone, Default)]
pub struct CondvarWaker(pub Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);

impl EventLoopWaker for CondvarWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        let (lock, cvar) = &*self.0;
        if let Ok(mut pending) = lock.lock() {
            *pending = true;
            cvar.notify_all();
        }
    }
}

impl CondvarWaker {
    /// Blocks until woken or `timeout_ms` elapses. Returns true if woken.
    pub fn wait_timeout(&self, timeout_ms: u64) -> bool {
        let (lock, cvar) = &*self.0;
        let pending = match lock.lock() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let woken = cvar
            .wait_timeout_while(pending, std::time::Duration::from_millis(timeout_ms), |p| !*p)
            .map(|(mut p, _)| {
                let was_pending = *p;
                *p = false;
                was_pending
            })
            .unwrap_or(false);
        woken
    }
}
