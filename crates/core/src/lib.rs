//! The crosspoint engine: a transport-agnostic N-source x M-output router.
//!
//! A [`Source`] is anything that produces a stream of payload chunks — today
//! that's only relayed SRT input, but the same registration API is meant to
//! be fed by non-relay sources later (a decoded/re-encoded media player,
//! stills, a scaler tap) without changing this crate. An [`Output`] always
//! carries exactly one source at a time, selected via [`Crosspoint::route`],
//! matching how a hardware video router behaves.
//!
//! Distribution uses a broadcast channel per source (fan-out to however many
//! outputs are currently pointed at it) and a watch channel per output (the
//! output task selects on it to notice routing changes and re-subscribe).

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{broadcast, watch};

pub type SourceId = String;
pub type OutputId = String;

/// How many payload chunks a source will buffer for a lagging output before
/// that output starts dropping frames instead of blocking the source.
const SOURCE_CHANNEL_CAPACITY: usize = 1024;

#[derive(Default)]
pub struct Crosspoint {
    sources: RwLock<HashMap<SourceId, broadcast::Sender<Bytes>>>,
    outputs: RwLock<HashMap<OutputId, watch::Sender<SourceId>>>,
}

impl Crosspoint {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a new source and get back the sender it should publish
    /// every payload chunk to. Re-registering an existing id replaces it.
    pub fn register_source(&self, id: impl Into<SourceId>) -> broadcast::Sender<Bytes> {
        let (tx, _rx) = broadcast::channel(SOURCE_CHANNEL_CAPACITY);
        self.sources.write().insert(id.into(), tx.clone());
        tx
    }

    /// Register a new output with the source it should start out routed
    /// from. Returns the watch::Receiver the output task should select on to
    /// learn about future routing changes.
    pub fn register_output(
        &self,
        id: impl Into<OutputId>,
        initial_source: impl Into<SourceId>,
    ) -> watch::Receiver<SourceId> {
        let (tx, rx) = watch::channel(initial_source.into());
        self.outputs.write().insert(id.into(), tx);
        rx
    }

    /// Get a fresh receiver for a source's payload stream, if it exists.
    pub fn subscribe(&self, source: &str) -> Option<broadcast::Receiver<Bytes>> {
        self.sources.read().get(source).map(|tx| tx.subscribe())
    }

    /// Remove a source. Any output currently routed to it keeps its `watch`
    /// value pointing at the now-gone id (routing is a separate concern,
    /// see [`route`](Self::route)) but its `subscribe` will start returning
    /// `None`/closed — the caller (a `*-io` crate's output task) is expected
    /// to treat that the same as "nothing routed yet" until re-routed.
    /// Returns `true` if a source with this id existed.
    pub fn deregister_source(&self, id: &str) -> bool {
        self.sources.write().remove(id).is_some()
    }

    /// Remove an output. Returns `true` if an output with this id existed.
    pub fn deregister_output(&self, id: &str) -> bool {
        self.outputs.write().remove(id).is_some()
    }

    pub fn has_source(&self, id: &str) -> bool {
        self.sources.read().contains_key(id)
    }

    pub fn has_output(&self, id: &str) -> bool {
        self.outputs.read().contains_key(id)
    }

    /// Point an output at a different source. No-ops (returns `false`) if
    /// either id is unknown, so callers (e.g. the web API) can distinguish
    /// a bad request from a successful re-route.
    pub fn route(&self, output: &str, source: &str) -> bool {
        if !self.sources.read().contains_key(source) {
            return false;
        }
        match self.outputs.read().get(output) {
            Some(tx) => {
                let _ = tx.send(source.to_string());
                true
            }
            None => false,
        }
    }

    pub fn source_ids(&self) -> Vec<SourceId> {
        self.sources.read().keys().cloned().collect()
    }

    pub fn output_ids(&self) -> Vec<OutputId> {
        self.outputs.read().keys().cloned().collect()
    }

    /// The current output -> source routing table.
    pub fn routes(&self) -> HashMap<OutputId, SourceId> {
        self.outputs
            .read()
            .iter()
            .map(|(output, tx)| (output.clone(), tx.borrow().clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_follows_route_changes() {
        let xp = Crosspoint::new();
        let tx_a = xp.register_source("a");
        let tx_b = xp.register_source("b");
        let mut route_rx = xp.register_output("out1", "a");

        let mut rx = xp.subscribe(&route_rx.borrow().clone()).unwrap();
        tx_a.send(Bytes::from_static(b"from-a")).unwrap();
        assert_eq!(rx.recv().await.unwrap(), Bytes::from_static(b"from-a"));

        assert!(xp.route("out1", "b"));
        route_rx.changed().await.unwrap();
        let mut rx = xp.subscribe(&route_rx.borrow().clone()).unwrap();
        tx_b.send(Bytes::from_static(b"from-b")).unwrap();
        assert_eq!(rx.recv().await.unwrap(), Bytes::from_static(b"from-b"));
    }

    #[test]
    fn route_rejects_unknown_ids() {
        let xp = Crosspoint::new();
        xp.register_source("a");
        xp.register_output("out1", "a");
        assert!(!xp.route("out1", "nonexistent"));
        assert!(!xp.route("nonexistent", "a"));
    }

    #[test]
    fn deregister_removes_and_reports_prior_existence() {
        let xp = Crosspoint::new();
        xp.register_source("a");
        xp.register_output("out1", "a");

        assert!(xp.has_source("a"));
        assert!(xp.has_output("out1"));

        assert!(xp.deregister_source("a"));
        assert!(!xp.has_source("a"));
        assert!(!xp.deregister_source("a"), "second removal reports false");

        assert!(xp.deregister_output("out1"));
        assert!(!xp.has_output("out1"));

        assert!(xp.subscribe("a").is_none());
    }
}
