//! NDI transport for the crosspoint: receives/sends NDI video, audio, and
//! metadata frames, carried through `crosspoint-core`'s `Bytes` broadcast
//! channel as a self-describing envelope (see [`envelope`]) rather than a
//! raw relayed byte stream — NDI has no single opaque payload the way SRT's
//! MPEG-TS does, each frame is a distinct, structured thing.
//!
//! Requires the real NDI SDK to build (this crate's `grafton-ndi` dependency
//! runs `bindgen` against the installed SDK headers) — see the workspace
//! README for how this feature is gated.
//!
//! NDI's own blocking capture/send calls run on dedicated blocking threads
//! (`tokio::task::spawn_blocking`) rather than mixed into the async
//! executor, and routing changes are noticed by polling the crosspoint's
//! `watch` channel every few milliseconds instead of `.await`ing it — the
//! same tradeoff `crates/router/src/state.rs` already makes for
//! persistence, just here because there's no async-friendly blocking
//! primitive for it either.

mod envelope;

use std::sync::Arc;
use std::time::Duration;

use crosspoint_core::Crosspoint;
use grafton_ndi::{
    Finder, FinderOptions, Receiver, ReceiverOptions, Sender, SenderOptions, Source, NDI,
};
use serde::Deserialize;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Endpoint {
    /// Connect to an NDI source on the network whose name contains this
    /// substring (NDI names look like `"MACHINE (Source Name)"` — matching
    /// on a substring avoids needing the exact machine-qualified name).
    Receiver { source_name: String },
    /// Advertise a new NDI output with this name.
    Sender { name: String },
}

const RETRY_DELAY: Duration = Duration::from_secs(2);
const SOURCE_DISCOVERY_POLL: Duration = Duration::from_secs(5);
const OUTPUT_ROUTE_POLL: Duration = Duration::from_millis(5);

#[derive(Debug, thiserror::Error)]
enum NdiIoError {
    #[error(transparent)]
    Ndi(#[from] grafton_ndi::Error),
    #[error("NDI source disconnected")]
    Disconnected,
}

/// Spawn the task for one NDI input: discover the named source, capture
/// video/audio/metadata frames, publish each as an envelope onto the
/// crosspoint's broadcast channel for `id`. Reconnects (re-discovers) on
/// disconnect. Runs until cancelled (or the process exits).
pub fn spawn_input(
    id: String,
    endpoint: Endpoint,
    crosspoint: Arc<Crosspoint>,
) -> CancellationToken {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let tx = crosspoint.register_source(id.clone());
    tokio::task::spawn_blocking(move || {
        let Endpoint::Receiver { source_name } = endpoint else {
            tracing::error!(source = %id, "ndi-io::spawn_input called with a Sender endpoint");
            return;
        };
        while !task_cancel.is_cancelled() {
            match run_receiver(&id, &source_name, &tx, &task_cancel) {
                Ok(()) => {}
                Err(err) => warn!(source = %id, %err, "NDI input error, retrying"),
            }
            if task_cancel.is_cancelled() {
                break;
            }
            std::thread::sleep(RETRY_DELAY);
        }
        info!(source = %id, "NDI input stopped");
    });
    cancel
}

/// Spawn the task for one NDI output: advertise `name` on the network,
/// forward whatever payload the crosspoint currently routes to `id` by
/// decoding each envelope and re-sending the equivalent NDI frame. Runs
/// until cancelled (or the process exits).
pub fn spawn_output(
    id: String,
    endpoint: Endpoint,
    default_source: String,
    crosspoint: Arc<Crosspoint>,
) -> CancellationToken {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let route_rx = crosspoint.register_output(id.clone(), default_source);
    tokio::task::spawn_blocking(move || {
        let Endpoint::Sender { name } = endpoint else {
            tracing::error!(output = %id, "ndi-io::spawn_output called with a Receiver endpoint");
            return;
        };
        while !task_cancel.is_cancelled() {
            match run_sender(&id, &name, route_rx.clone(), &crosspoint, &task_cancel) {
                Ok(()) => {}
                Err(err) => warn!(output = %id, %err, "NDI output error, retrying"),
            }
            if task_cancel.is_cancelled() {
                break;
            }
            std::thread::sleep(RETRY_DELAY);
        }
        info!(output = %id, "NDI output stopped");
    });
    cancel
}

fn find_source_by_name(finder: &Finder, wanted: &str) -> Result<Source, NdiIoError> {
    loop {
        let sources = finder.current_sources()?;
        if let Some(found) = sources.iter().find(|s| s.name.contains(wanted)) {
            return Ok(found.clone());
        }
        finder.wait_for_sources(SOURCE_DISCOVERY_POLL)?;
    }
}

fn run_receiver(
    id: &str,
    source_name: &str,
    tx: &broadcast::Sender<bytes::Bytes>,
    cancel: &CancellationToken,
) -> Result<(), NdiIoError> {
    let ndi = NDI::new()?;
    let finder = Finder::new(
        &ndi,
        &FinderOptions::builder().show_local_sources(true).build(),
    )?;
    info!(source = %id, wanted = %source_name, "searching for NDI source");
    let source = find_source_by_name(&finder, source_name)?;
    info!(source = %id, ndi_source = %source.name, "NDI input connected");

    let options = ReceiverOptions::builder(source).build();
    let receiver = Receiver::new(&ndi, &options)?;

    // Deliberately *not* gating on `receiver.is_connected()` per-iteration:
    // it reads false for a while right after connecting (a fresh socket
    // with nothing sent yet, not an actual drop), and empirically —
    // confirmed against this crate's own sustained-capture integration
    // test — it can keep reading false well after frames are flowing
    // normally. Treating that as fatal caused spurious reconnect-storms
    // that starved the receiver of the time it needed to settle. Instead,
    // only reconnect if literally nothing has arrived (any of
    // video/audio/metadata) for a long time — long enough that a real,
    // live source could not plausibly go that quiet.
    let mut last_frame_at = std::time::Instant::now();
    const SILENCE_TIMEOUT: Duration = Duration::from_secs(30);

    while !cancel.is_cancelled() {
        let mut got_frame = false;
        if let Some(frame) = receiver.video().try_capture(Duration::from_millis(200))? {
            let _ = tx.send(envelope::encode_video(&frame));
            got_frame = true;
        }
        if let Some(frame) = receiver.audio().try_capture(Duration::from_millis(1))? {
            let _ = tx.send(envelope::encode_audio(&frame));
            got_frame = true;
        }
        if let Some(frame) = receiver.metadata().try_capture(Duration::from_millis(1))? {
            let _ = tx.send(envelope::encode_metadata(&frame));
            got_frame = true;
        }
        if got_frame {
            last_frame_at = std::time::Instant::now();
        } else if last_frame_at.elapsed() > SILENCE_TIMEOUT {
            return Err(NdiIoError::Disconnected);
        }
    }
    Ok(())
}

fn run_sender(
    id: &str,
    name: &str,
    mut route_rx: watch::Receiver<String>,
    crosspoint: &Arc<Crosspoint>,
    cancel: &CancellationToken,
) -> Result<(), NdiIoError> {
    let ndi = NDI::new()?;
    let sender = Sender::new(&ndi, &SenderOptions::builder(name).build())?;
    info!(output = %id, ndi_name = %name, "NDI output advertising");

    while !cancel.is_cancelled() {
        let current = route_rx.borrow_and_update().clone();
        let Some(mut rx) = crosspoint.subscribe(&current) else {
            std::thread::sleep(OUTPUT_ROUTE_POLL);
            continue;
        };
        while !cancel.is_cancelled() {
            if route_rx.has_changed().unwrap_or(false) {
                break; // re-subscribe to the newly routed source
            }
            match rx.try_recv() {
                Ok(bytes) => match envelope::decode(bytes) {
                    Ok(envelope::DecodedFrame::Video(frame)) => sender.send_video(&frame),
                    Ok(envelope::DecodedFrame::Audio(frame)) => sender.send_audio(&frame),
                    Ok(envelope::DecodedFrame::Metadata(frame)) => {
                        if let Err(err) = sender.send_metadata(&frame) {
                            warn!(output = %id, %err, "NDI metadata send failed");
                        }
                    }
                    Err(err) => warn!(output = %id, %err, "failed to decode NDI envelope"),
                },
                Err(broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(OUTPUT_ROUTE_POLL);
                }
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    warn!(output = %id, skipped, "NDI output lagged, dropped frames");
                }
                Err(broadcast::error::TryRecvError::Closed) => return Ok(()),
            }
        }
    }
    Ok(())
}
