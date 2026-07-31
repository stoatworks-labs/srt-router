//! NDI transport for the crosspoint: receives/sends NDI video, audio, and
//! metadata frames, carried through `crosspoint-core`'s `Bytes` broadcast
//! channel as a self-describing envelope (see [`envelope`]) rather than a
//! raw relayed byte stream — NDI has no single opaque payload the way SRT's
//! MPEG-TS does, each frame is a distinct, structured thing.
//!
//! Needs no NDI SDK to build: [`sys`] loads the runtime with `dlopen` at run
//! time. A machine with no runtime still builds and runs the router — only an
//! NDI endpoint fails, with the download URL in the message. This replaced
//! `grafton-ndi`, whose build-time link made the crate unbuildable without the
//! proprietary SDK and so kept NDI out of every cross-compiled release.
//!
//! NDI's own blocking capture/send calls run on dedicated blocking threads
//! (`tokio::task::spawn_blocking`) rather than mixed into the async
//! executor, and routing changes are noticed by polling the crosspoint's
//! `watch` channel every few milliseconds instead of `.await`ing it — the
//! same tradeoff `crates/router/src/state.rs` already makes for
//! persistence, just here because there's no async-friendly blocking
//! primitive for it either.

mod envelope;
pub mod sys;

use std::sync::Arc;
use std::time::Duration;

use crosspoint_core::Crosspoint;
use serde::Deserialize;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::sys::{Finder, Frame, Receiver, Sender, Source};

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
/// How long one `recv_capture_v3` waits. Short enough that cancellation is
/// noticed promptly, long enough not to spin.
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
enum NdiIoError {
    /// Every runtime-level failure, including "not installed" — `sys` puts the
    /// download URL in that one's message, so nothing is needed here.
    #[error(transparent)]
    Ndi(#[from] sys::SysError),
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

/// `Ok(None)` means cancelled before a match ever appeared — distinct from
/// an `Err`, which means the NDI SDK itself failed. Not returning `Err` for
/// cancellation keeps the caller's "reconnect on real errors" retry loop
/// from treating a clean shutdown as a failure worth logging a warning for.
fn find_source_by_name(
    finder: &mut Finder,
    wanted: &str,
    cancel: &CancellationToken,
) -> Option<Source> {
    while !cancel.is_cancelled() {
        if let Some(found) = finder
            .current_sources()
            .into_iter()
            .find(|s| s.name.contains(wanted))
        {
            return Some(found);
        }
        finder.wait_for_sources(SOURCE_DISCOVERY_POLL);
    }
    None
}

fn run_receiver(
    id: &str,
    source_name: &str,
    tx: &broadcast::Sender<bytes::Bytes>,
    cancel: &CancellationToken,
) -> Result<(), NdiIoError> {
    let api = sys::load()?;
    let mut finder = Finder::new(api.clone(), true)?;
    info!(source = %id, wanted = %source_name, "searching for NDI source");
    let Some(source) = find_source_by_name(&mut finder, source_name, cancel) else {
        return Ok(()); // cancelled while still searching
    };
    info!(source = %id, ndi_source = %source.name, "NDI input connected");

    let mut receiver = Receiver::connect(api, &source, "srt-router")?;

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
        // One call serves all three kinds: `recv_capture_v3` returns whichever
        // frame arrived first, so unlike the previous three-call poll there is
        // no per-kind timeout to balance.
        let got_frame = match receiver.capture(CAPTURE_TIMEOUT) {
            Some(Frame::Video(frame)) => {
                let _ = tx.send(envelope::encode_video(&frame));
                true
            }
            Some(Frame::Audio(frame)) => {
                let _ = tx.send(envelope::encode_audio(&frame));
                true
            }
            Some(Frame::Metadata(frame)) => {
                let _ = tx.send(envelope::encode_metadata(&frame));
                true
            }
            None => false,
        };
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
    let api = sys::load()?;
    let mut sender = Sender::new(api, name)?;
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
