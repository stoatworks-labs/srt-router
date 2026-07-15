//! SRT transport for the crosspoint: relays SRT payload chunks straight
//! between the network and a [`crosspoint_core::Crosspoint`], with no
//! decode/re-encode. Each input and output owns exactly one SRT socket and
//! reconnects on its own (listener: re-listen; caller: re-dial) whenever the
//! connection drops — a router should keep trying, not need a restart
//! because one encoder blipped.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use crosspoint_core::Crosspoint;
use futures::SinkExt;
use serde::Deserialize;
use srt_tokio::SrtSocket;
use tokio::sync::broadcast;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{info, warn};

/// How an SRT socket for one endpoint is established.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Endpoint {
    /// This router waits for the remote encoder/decoder to connect to it.
    Listener { bind: String },
    /// This router dials out to a remote SRT listener.
    Caller { connect: String },
}

/// Delay between reconnect attempts. Fixed rather than exponential-backoff:
/// a live-video router's job is to reacquire as soon as the far end is back,
/// and a fixed short interval is what real routers/decoders do.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

async fn open(endpoint: &Endpoint) -> std::io::Result<SrtSocket> {
    match endpoint {
        Endpoint::Listener { bind } => SrtSocket::builder().listen_on(bind.as_str()).await,
        Endpoint::Caller { connect } => SrtSocket::builder().call(connect.as_str(), None).await,
    }
}

/// Spawn the task for one SRT input: connect (or wait for a connection),
/// publish every payload chunk onto the crosspoint's broadcast channel for
/// `id`, and reconnect on drop. Runs until the process exits.
pub fn spawn_input(id: String, endpoint: Endpoint, crosspoint: Arc<Crosspoint>) {
    let tx = crosspoint.register_source(id.clone());
    tokio::spawn(async move {
        loop {
            match open(&endpoint).await {
                Ok(mut socket) => {
                    info!(source = %id, "SRT input connected");
                    relay_in(&mut socket, &tx, &id).await;
                    warn!(source = %id, "SRT input disconnected, reconnecting");
                }
                Err(err) => {
                    warn!(source = %id, %err, "SRT input connect failed, retrying");
                }
            }
            sleep(RECONNECT_DELAY).await;
        }
    });
}

/// Spawn the task for one SRT output: connect (or wait for a connection),
/// forward whatever payload the crosspoint currently routes to `id`, and
/// reconnect on drop or re-subscribe on a routing change. Runs until the
/// process exits.
pub fn spawn_output(
    id: String,
    endpoint: Endpoint,
    default_source: String,
    crosspoint: Arc<Crosspoint>,
) {
    let route_rx = crosspoint.register_output(id.clone(), default_source);
    tokio::spawn(async move {
        loop {
            match open(&endpoint).await {
                Ok(mut socket) => {
                    info!(output = %id, "SRT output connected");
                    relay_out(&mut socket, route_rx.clone(), &crosspoint, &id).await;
                    warn!(output = %id, "SRT output disconnected, reconnecting");
                }
                Err(err) => {
                    warn!(output = %id, %err, "SRT output connect failed, retrying");
                }
            }
            sleep(RECONNECT_DELAY).await;
        }
    });
}

async fn relay_in(socket: &mut SrtSocket, tx: &broadcast::Sender<Bytes>, id: &str) {
    loop {
        match socket.try_next().await {
            Ok(Some((_instant, bytes))) => {
                // No receivers is normal (no output currently routed here) —
                // send() erroring in that case isn't a problem.
                let _ = tx.send(bytes);
            }
            Ok(None) => return,
            Err(err) => {
                warn!(source = %id, %err, "SRT input read error");
                return;
            }
        }
    }
}

async fn relay_out(
    socket: &mut SrtSocket,
    mut route_rx: tokio::sync::watch::Receiver<String>,
    crosspoint: &Arc<Crosspoint>,
    id: &str,
) {
    loop {
        let current = route_rx.borrow().clone();
        let Some(mut rx) = crosspoint.subscribe(&current) else {
            // Routed at a source id that isn't registered (shouldn't happen
            // with config-validated ids); wait for the route to change.
            if route_rx.changed().await.is_err() {
                return;
            }
            continue;
        };
        loop {
            tokio::select! {
                changed = route_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
                frame = rx.recv() => {
                    match frame {
                        Ok(bytes) => {
                            if let Err(err) = socket.send((Instant::now(), bytes)).await {
                                warn!(output = %id, %err, "SRT output write error");
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(output = %id, skipped, "SRT output lagged, dropped frames");
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        }
    }
}
