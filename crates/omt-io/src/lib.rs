//! OMT transport for the crosspoint — the second, genuinely-open
//! (MIT-licensed) frame-based transport alongside `ndi-io`. Same shape as
//! `ndi-io` throughout: `spawn_input`/`spawn_output`, blocking SDK calls on
//! dedicated `spawn_blocking` threads, a `CancellationToken` per task, and
//! an envelope (`envelope.rs`) carrying frames through `crosspoint-core`'s
//! plain `Bytes` channel.
//!
//! Requires the real OMT SDK to build — see `build.rs` for how `OMT_LIB_DIR`
//! is used, since (unlike NDI) there's no standard system install location
//! to fall back to.

mod envelope;
/// `pub` so the integration test (`tests/relay.rs`) can act as a real,
/// independent OMT sender/receiver the same way `crates/ndi-io`'s test
/// uses `grafton_ndi` directly — exercising this crate's envelope
/// encode/decode against genuine external OMT traffic, not just its own
/// round trip. Not meant as a stable public API; downstream code should
/// use `Endpoint`/`spawn_input`/`spawn_output` instead.
pub mod sys;

use std::ffi::{CStr, CString};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use crosspoint_core::Crosspoint;
use serde::Deserialize;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Endpoint {
    /// Connect to an OMT source on the network whose discovery address
    /// contains this substring (addresses look like `"HOSTNAME (Name)"`).
    Receiver { address: String },
    /// Advertise a new OMT output with this name.
    Sender { name: String },
}

const RETRY_DELAY: Duration = Duration::from_secs(2);
const DISCOVERY_POLL: Duration = Duration::from_secs(2);
const RECEIVE_TIMEOUT_MS: i32 = 200;
const SILENCE_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_ROUTE_POLL: Duration = Duration::from_millis(5);

#[derive(Debug, thiserror::Error)]
enum OmtIoError {
    #[error("OMT source disconnected or never appeared")]
    Disconnected,
    #[error("omt_receive_create failed for '{0}'")]
    ReceiveCreateFailed(String),
    #[error("omt_send_create failed for '{0}'")]
    SendCreateFailed(String),
}

/// `Ok(None)` means cancelled before a match ever appeared — see
/// `ndi-io`'s identical `find_source_by_name` for why this isn't an `Err`.
fn find_address_by_name(wanted: &str, cancel: &CancellationToken) -> Option<String> {
    while !cancel.is_cancelled() {
        let addrs = list_addresses();
        if let Some(found) = addrs.iter().find(|a| a.contains(wanted)) {
            return Some(found.clone());
        }
        std::thread::sleep(DISCOVERY_POLL);
    }
    None
}

fn list_addresses() -> Vec<String> {
    // SAFETY: `omt_discovery_getaddresses` returns a library-owned array
    // valid until the next call to it — we copy every string out to an
    // owned `String` immediately and never hold the raw pointers past this
    // function, so there's no dangling-pointer or double-free risk here.
    unsafe {
        let mut count: std::ffi::c_int = 0;
        let ptr = sys::omt_discovery_getaddresses(&mut count);
        if ptr.is_null() || count <= 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(ptr, count as usize)
            .iter()
            .filter(|p| !p.is_null())
            .map(|&p| CStr::from_ptr(p).to_string_lossy().into_owned())
            .collect()
    }
}

/// Wraps an `omt_receive_t*` so it's destroyed exactly once, however the
/// enclosing function returns (including on an early `?`/error path).
struct ReceiveHandle(*mut sys::omt_receive_t);
impl Drop for ReceiveHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { sys::omt_receive_destroy(self.0) };
        }
    }
}
// SAFETY: the OMT SDK's receive/send instances are documented as usable
// from a single thread at a time, which is exactly how this crate uses
// them (one dedicated blocking thread owns each handle for its entire
// lifetime) — Send is required only to move the handle into that thread's
// closure at spawn time, not to share it concurrently.
unsafe impl Send for ReceiveHandle {}

struct SendHandle(*mut sys::omt_send_t);
impl Drop for SendHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { sys::omt_send_destroy(self.0) };
        }
    }
}
unsafe impl Send for SendHandle {}

/// Spawn the task for one OMT input: discover the named source, receive
/// video/audio/metadata frames, publish each as an envelope onto the
/// crosspoint's broadcast channel for `id`. Runs until cancelled.
pub fn spawn_input(
    id: String,
    endpoint: Endpoint,
    crosspoint: Arc<Crosspoint>,
) -> CancellationToken {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let tx = crosspoint.register_source(id.clone());
    tokio::task::spawn_blocking(move || {
        let Endpoint::Receiver { address } = endpoint else {
            tracing::error!(source = %id, "omt-io::spawn_input called with a Sender endpoint");
            return;
        };
        while !task_cancel.is_cancelled() {
            match run_receiver(&id, &address, &tx, &task_cancel) {
                Ok(()) => {}
                Err(err) => warn!(source = %id, %err, "OMT input error, retrying"),
            }
            if task_cancel.is_cancelled() {
                break;
            }
            std::thread::sleep(RETRY_DELAY);
        }
        info!(source = %id, "OMT input stopped");
    });
    cancel
}

/// Spawn the task for one OMT output: advertise `name`, forward whatever
/// payload the crosspoint currently routes to `id`. Runs until cancelled.
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
            tracing::error!(output = %id, "omt-io::spawn_output called with a Receiver endpoint");
            return;
        };
        while !task_cancel.is_cancelled() {
            match run_sender(&id, &name, route_rx.clone(), &crosspoint, &task_cancel) {
                Ok(()) => {}
                Err(err) => warn!(output = %id, %err, "OMT output error, retrying"),
            }
            if task_cancel.is_cancelled() {
                break;
            }
            std::thread::sleep(RETRY_DELAY);
        }
        info!(output = %id, "OMT output stopped");
    });
    cancel
}

fn run_receiver(
    id: &str,
    wanted: &str,
    tx: &broadcast::Sender<Bytes>,
    cancel: &CancellationToken,
) -> Result<(), OmtIoError> {
    info!(source = %id, wanted, "searching for OMT source");
    let Some(address) = find_address_by_name(wanted, cancel) else {
        return Ok(()); // cancelled while still searching
    };
    info!(source = %id, omt_address = %address, "OMT input connected");

    let address_c = CString::new(address).unwrap_or_default();
    let raw = unsafe {
        sys::omt_receive_create(
            address_c.as_ptr(),
            sys::OMTFrameTypeMask::ALL,
            sys::OMTPreferredVideoFormat::UYVY,
            sys::OMTReceiveFlags::None,
        )
    };
    if raw.is_null() {
        return Err(OmtIoError::ReceiveCreateFailed(id.to_string()));
    }
    let handle = ReceiveHandle(raw);

    let mut last_frame_at = std::time::Instant::now();
    while !cancel.is_cancelled() {
        // SAFETY: `handle.0` is non-null (checked above) and owned by this
        // function for its whole lifetime. The returned pointer, if
        // non-null, points at data valid only until the next call to
        // `omt_receive` on this instance — we read every field and copy
        // `data` into an owned `Vec` before looping back, never retaining
        // the pointer past this iteration.
        let frame_ptr =
            unsafe { sys::omt_receive(handle.0, sys::OMTFrameTypeMask::ALL, RECEIVE_TIMEOUT_MS) };
        if frame_ptr.is_null() {
            if last_frame_at.elapsed() > SILENCE_TIMEOUT {
                return Err(OmtIoError::Disconnected);
            }
            continue;
        }
        let frame = unsafe { *frame_ptr };
        if frame.frame_type == 0 {
            continue; // OMTFrameType_None
        }
        let data: Vec<u8> = if frame.data.is_null() || frame.data_length <= 0 {
            Vec::new()
        } else {
            unsafe {
                std::slice::from_raw_parts(frame.data as *const u8, frame.data_length as usize)
                    .to_vec()
            }
        };
        let _ = tx.send(envelope::encode(&frame, &data));
        last_frame_at = std::time::Instant::now();
    }
    Ok(())
}

fn run_sender(
    id: &str,
    name: &str,
    mut route_rx: watch::Receiver<String>,
    crosspoint: &Arc<Crosspoint>,
    cancel: &CancellationToken,
) -> Result<(), OmtIoError> {
    let name_c = CString::new(name).unwrap_or_default();
    let raw = unsafe { sys::omt_send_create(name_c.as_ptr(), sys::OMTQuality::Default) };
    if raw.is_null() {
        return Err(OmtIoError::SendCreateFailed(id.to_string()));
    }
    let handle = SendHandle(raw);
    info!(output = %id, omt_name = %name, "OMT output advertising");

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
                Ok(bytes) => {
                    let Some(decoded) = envelope::decode(bytes) else {
                        warn!(output = %id, "failed to decode OMT envelope, skipping frame");
                        continue;
                    };
                    let mut data = decoded.data;
                    let mut frame = sys::OMTMediaFrame {
                        frame_type: decoded.frame_type,
                        timestamp: decoded.timestamp,
                        codec: decoded.codec,
                        width: decoded.width,
                        height: decoded.height,
                        stride: decoded.stride,
                        flags: decoded.flags,
                        frame_rate_n: decoded.frame_rate_n,
                        frame_rate_d: decoded.frame_rate_d,
                        aspect_ratio: decoded.aspect_ratio,
                        color_space: decoded.color_space,
                        sample_rate: decoded.sample_rate,
                        channels: decoded.channels,
                        samples_per_channel: decoded.samples_per_channel,
                        data: data.as_mut_ptr() as *mut std::ffi::c_void,
                        data_length: data.len() as i32,
                        ..Default::default()
                    };
                    // SAFETY: `frame.data` points into `data`, which
                    // outlives this call (dropped only after `omt_send`
                    // returns, at the end of this match arm) — `omt_send`
                    // is documented to process the frame synchronously,
                    // the same contract NDI's send_video relies on.
                    let sent = unsafe { sys::omt_send(handle.0, &mut frame) };
                    if sent == 0 {
                        warn!(output = %id, "omt_send reported failure");
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(OUTPUT_ROUTE_POLL);
                }
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    warn!(output = %id, skipped, "OMT output lagged, dropped frames");
                }
                Err(broadcast::error::TryRecvError::Closed) => return Ok(()),
            }
        }
    }
    Ok(())
}
