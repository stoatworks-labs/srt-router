//! Tracks the transport kind + teardown handle for every source/output
//! currently running, whether it came from the static config at startup or
//! was added later via the management API — both go through the same
//! `insert_*`/`remove_*` calls, so both are equally listable and removable.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;

pub struct Entry {
    pub kind: &'static str,
    pub cancel: CancellationToken,
}

#[derive(Default)]
pub struct Registry {
    sources: RwLock<HashMap<String, Entry>>,
    outputs: RwLock<HashMap<String, Entry>>,
}

impl Registry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn insert_source(&self, id: String, kind: &'static str, cancel: CancellationToken) {
        self.sources.write().insert(id, Entry { kind, cancel });
    }

    pub fn insert_output(&self, id: String, kind: &'static str, cancel: CancellationToken) {
        self.outputs.write().insert(id, Entry { kind, cancel });
    }

    /// Remove and return the entry so the caller can cancel its task. Kept
    /// separate from cancellation itself so the registry stays free of any
    /// crosspoint-specific teardown logic (deregistering the id) — that's
    /// the caller's job, see `management.rs`.
    pub fn remove_source(&self, id: &str) -> Option<Entry> {
        self.sources.write().remove(id)
    }

    pub fn remove_output(&self, id: &str) -> Option<Entry> {
        self.outputs.write().remove(id)
    }

    pub fn sources(&self) -> HashMap<String, &'static str> {
        self.sources
            .read()
            .iter()
            .map(|(id, e)| (id.clone(), e.kind))
            .collect()
    }

    pub fn outputs(&self) -> HashMap<String, &'static str> {
        self.outputs
            .read()
            .iter()
            .map(|(id, e)| (id.clone(), e.kind))
            .collect()
    }

    /// The transport kind of `id`, checking sources then outputs. Used to
    /// reject cross-kind routes (see `crosspoint_web::app_with_kind_lookup`)
    /// — a single lookup covering both is enough there since source ids and
    /// output ids are just whatever the caller named them, not namespaced.
    pub fn kind_of(&self, id: &str) -> Option<&'static str> {
        self.sources
            .read()
            .get(id)
            .map(|e| e.kind)
            .or_else(|| self.outputs.read().get(id).map(|e| e.kind))
    }
}
