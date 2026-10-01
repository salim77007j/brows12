//! Shared handle to the live document, safe for the JS thread.

use brows12_html::Document;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The JS bindings operate on the engine's document through this handle.
/// A mutation flag lets the engine re-run style/layout/render only when
/// scripts actually touched the tree.
#[derive(Clone)]
pub struct DomHandle {
    document: Arc<Mutex<Document>>,
    mutated: Arc<AtomicBool>,
}

impl std::fmt::Debug for DomHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DomHandle").field("mutated", &self.mutated.load(Ordering::Relaxed)).finish()
    }
}

impl DomHandle {
    pub fn new(document: Arc<Mutex<Document>>) -> Self {
        Self { document, mutated: Arc::new(AtomicBool::new(false)) }
    }

    /// Read-only access to the document.
    pub fn read<F, T>(&self, f: F) -> Option<T>
    where
        F: FnOnce(&Document) -> T,
    {
        let guard = self.document.lock().ok()?;
        Some(f(&guard))
    }

    /// Mutating access; automatically flags the document for re-render.
    pub fn mutate<F, T>(&self, f: F) -> Option<T>
    where
        F: FnOnce(&mut Document) -> T,
    {
        let mut guard = self.document.lock().ok()?;
        self.mutated.store(true, Ordering::Relaxed);
        Some(f(&mut guard))
    }

    /// Mark the document as script-mutated without touching the tree.
    pub fn mark_mutated(&self) {
        self.mutated.store(true, Ordering::Relaxed);
    }

    /// Consume the mutation flag.
    pub fn take_mutated(&self) -> bool {
        self.mutated.swap(false, Ordering::Relaxed)
    }
}
