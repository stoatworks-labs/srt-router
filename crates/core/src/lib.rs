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
//!
//! Each source also reports its [`SourceHealth`]: whether its transport is
//! connected, and whether payload is actually arriving. The two halves come
//! from different places on purpose. Only the `*-io` crate knows when its
//! socket/receiver/ffmpeg is up, so it holds a [`ConnectedGuard`] for as long
//! as it is; but every chunk goes through [`SourceSender::send`], so "is data
//! flowing" is measured here, once, the same way for every transport.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{broadcast, watch};

pub type SourceId = String;
pub type OutputId = String;

/// How many payload chunks a source will buffer for a lagging output before
/// that output starts dropping frames instead of blocking the source.
const SOURCE_CHANNEL_CAPACITY: usize = 1024;

/// How long a connected source may go without a single payload chunk before
/// it reads as [`SourceHealth::Idle`] instead of [`SourceHealth::Live`].
/// The one definition of "no data" for every transport. A live SRT feed
/// sends packets continuously (null packets included) and an NDI/OMT source
/// sends at least one frame per field, so two seconds of nothing means the
/// far end is connected but not sending, not that it is merely between frames.
pub const IDLE_AFTER: Duration = Duration::from_secs(2);

/// What a source is doing right now, as the web UI's dot and
/// `GET /api/state`'s `source_health` show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceHealth {
    /// Connected, and payload arrived within [`IDLE_AFTER`].
    Live,
    /// Connected, but nothing has arrived for at least [`IDLE_AFTER`].
    Idle,
    /// Not connected: an SRT listener still waiting for its caller, a caller
    /// still dialling, an NDI/OMT source not found yet, or an ffmpeg source
    /// between restarts.
    Offline,
}

impl SourceHealth {
    /// The wire name: `"live"`, `"idle"` or `"offline"`.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceHealth::Live => "live",
            SourceHealth::Idle => "idle",
            SourceHealth::Offline => "offline",
        }
    }
}

/// Shared between a source's [`SourceSender`] and the crosspoint. Plain
/// atomics, because `send` runs on every payload chunk and must not lock.
struct Activity {
    epoch: Instant,
    /// How many [`ConnectedGuard`]s are alive. A count rather than a flag so
    /// an overlapping pair (old connection's guard dropped after the new
    /// one's was taken) can't mark a connected source offline.
    connections: AtomicUsize,
    /// Nanoseconds after `epoch` at which the latest chunk arrived, plus one,
    /// so that 0 can mean "nothing since the current connection opened".
    last_chunk: AtomicU64,
}

impl Activity {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            connections: AtomicUsize::new(0),
            last_chunk: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        let nanos = u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX - 1);
        self.last_chunk.store(nanos + 1, Ordering::Relaxed);
    }

    fn health_at(&self, now: Instant) -> SourceHealth {
        if self.connections.load(Ordering::Relaxed) == 0 {
            return SourceHealth::Offline;
        }
        match self.last_chunk.load(Ordering::Relaxed) {
            0 => SourceHealth::Idle,
            stamp => {
                let at = self.epoch + Duration::from_nanos(stamp - 1);
                if now.saturating_duration_since(at) < IDLE_AFTER {
                    SourceHealth::Live
                } else {
                    SourceHealth::Idle
                }
            }
        }
    }
}

/// What a source publishes through: a broadcast sender that also records
/// that data is flowing. Returned by [`Crosspoint::register_source`].
#[derive(Clone)]
pub struct SourceSender {
    tx: broadcast::Sender<Bytes>,
    activity: Arc<Activity>,
}

impl SourceSender {
    /// Publish one payload chunk to every output currently routed here.
    /// Erroring when no output is routed here is normal, exactly as for
    /// `broadcast::Sender::send`; the chunk still counts as data arriving.
    pub fn send(&self, chunk: Bytes) -> Result<usize, broadcast::error::SendError<Bytes>> {
        self.activity.touch();
        self.tx.send(chunk)
    }

    /// Mark this source connected until the returned guard is dropped. Take
    /// it when the transport comes up (socket accepted, receiver created,
    /// ffmpeg started) and let it fall out of scope when that connection
    /// ends, on every path, error returns included.
    #[must_use = "the source reads as connected only while the guard is alive"]
    pub fn connected(&self) -> ConnectedGuard {
        if self.activity.connections.fetch_add(1, Ordering::Relaxed) == 0 {
            // Data from a previous connection says nothing about this one.
            self.activity.last_chunk.store(0, Ordering::Relaxed);
        }
        ConnectedGuard(self.activity.clone())
    }
}

/// See [`SourceSender::connected`].
pub struct ConnectedGuard(Arc<Activity>);

impl Drop for ConnectedGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

struct SourceEntry {
    tx: broadcast::Sender<Bytes>,
    activity: Arc<Activity>,
}

#[derive(Default)]
pub struct Crosspoint {
    sources: RwLock<HashMap<SourceId, SourceEntry>>,
    outputs: RwLock<HashMap<OutputId, watch::Sender<SourceId>>>,
}

impl Crosspoint {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a new source and get back the sender it should publish
    /// every payload chunk to. Re-registering an existing id replaces it.
    /// The source reads as [`SourceHealth::Offline`] until its `*-io` task
    /// takes a [`SourceSender::connected`] guard.
    pub fn register_source(&self, id: impl Into<SourceId>) -> SourceSender {
        let (tx, _rx) = broadcast::channel(SOURCE_CHANNEL_CAPACITY);
        let activity = Arc::new(Activity::new());
        self.sources.write().insert(
            id.into(),
            SourceEntry {
                tx: tx.clone(),
                activity: activity.clone(),
            },
        );
        SourceSender { tx, activity }
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
        self.sources.read().get(source).map(|e| e.tx.subscribe())
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

    /// Every source's [`SourceHealth`] as of now.
    pub fn source_health(&self) -> HashMap<SourceId, SourceHealth> {
        self.source_health_at(Instant::now())
    }

    fn source_health_at(&self, now: Instant) -> HashMap<SourceId, SourceHealth> {
        self.sources
            .read()
            .iter()
            .map(|(id, e)| (id.clone(), e.activity.health_at(now)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(xp: &Crosspoint, id: &str, now: Instant) -> SourceHealth {
        xp.source_health_at(now)[id]
    }

    #[test]
    fn a_new_source_is_offline_until_connected() {
        let xp = Crosspoint::new();
        let tx = xp.register_source("cam1");
        assert_eq!(health(&xp, "cam1", Instant::now()), SourceHealth::Offline);

        // Data without a connection (shouldn't happen, but) is still offline:
        // the transport, not the byte count, says whether the far end is there.
        let _ = tx.send(Bytes::from_static(b"x"));
        assert_eq!(health(&xp, "cam1", Instant::now()), SourceHealth::Offline);
    }

    #[test]
    fn connected_with_no_data_yet_is_idle_then_live_once_data_arrives() {
        let xp = Crosspoint::new();
        let tx = xp.register_source("cam1");
        let _up = tx.connected();
        assert_eq!(health(&xp, "cam1", Instant::now()), SourceHealth::Idle);

        // No output is routed here, so send() errors; it must still count.
        assert!(tx.send(Bytes::from_static(b"x")).is_err());
        assert_eq!(health(&xp, "cam1", Instant::now()), SourceHealth::Live);
    }

    #[test]
    fn data_going_quiet_turns_live_into_idle_after_the_threshold() {
        let xp = Crosspoint::new();
        let tx = xp.register_source("cam1");
        let _up = tx.connected();
        let _ = tx.send(Bytes::from_static(b"x"));
        let sent = Instant::now();

        let just_before = sent + IDLE_AFTER - Duration::from_millis(100);
        let just_after = sent + IDLE_AFTER + Duration::from_millis(100);
        assert_eq!(health(&xp, "cam1", just_before), SourceHealth::Live);
        assert_eq!(health(&xp, "cam1", just_after), SourceHealth::Idle);
    }

    #[test]
    fn dropping_the_guard_goes_offline_and_a_reconnect_starts_idle() {
        let xp = Crosspoint::new();
        let tx = xp.register_source("cam1");
        let up = tx.connected();
        let _ = tx.send(Bytes::from_static(b"x"));
        drop(up);
        assert_eq!(health(&xp, "cam1", Instant::now()), SourceHealth::Offline);

        // The chunk above belonged to the old connection; the new one has
        // sent nothing yet, so it must not inherit "live".
        let _up = tx.connected();
        assert_eq!(health(&xp, "cam1", Instant::now()), SourceHealth::Idle);
    }

    #[test]
    fn overlapping_guards_stay_connected_until_the_last_is_dropped() {
        let xp = Crosspoint::new();
        let tx = xp.register_source("cam1");
        let old = tx.connected();
        let new = tx.connected();
        drop(old);
        assert_ne!(health(&xp, "cam1", Instant::now()), SourceHealth::Offline);
        drop(new);
        assert_eq!(health(&xp, "cam1", Instant::now()), SourceHealth::Offline);
    }

    #[test]
    fn health_covers_exactly_the_registered_sources() {
        let xp = Crosspoint::new();
        xp.register_source("a");
        xp.register_source("b");
        xp.deregister_source("a");
        let all = xp.source_health();
        assert_eq!(all.len(), 1);
        assert_eq!(all["b"], SourceHealth::Offline);
    }

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
