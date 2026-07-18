//! Special-purpose, non-relay sources for the crosspoint: a looped still
//! image, a local media file, and a scaler tap that decodes/rescales/
//! re-encodes another registered source into a new one. Each spawns
//! `ffmpeg` as a child process and publishes its `pipe:1` MPEG-TS output
//! into the crosspoint exactly like `srt-io`'s relay does — from the
//! crosspoint's point of view these are ordinary `Bytes`-producing
//! sources, no different from an SRT input. The scaler additionally reads
//! its upstream source's own `Bytes` off the crosspoint and feeds them to
//! ffmpeg's `pipe:0`, making it the one case here that both consumes and
//! produces a source.
//!
//! Deliberately doesn't ask ffmpeg to speak SRT itself: plenty of ffmpeg
//! builds (this project's own dev machine's Homebrew build included) are
//! compiled without libsrt support (`ffmpeg -protocols` has no `srt`
//! entry), so `-f mpegts pipe:1` is used instead — works with any baseline
//! ffmpeg install and skips a network hop entirely, since the bytes are
//! already local to this process.
//!
//! Unlike `ndi-io`/`omt-io`, there's no proprietary SDK or build-time
//! linking involved — `ffmpeg` is a runtime dependency, checked by
//! actually trying to spawn it. So this crate carries no Cargo feature
//! gate and is a normal `default-members` workspace member.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use crosspoint_core::Crosspoint;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, Command};
use tokio::sync::broadcast;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// How a media-io source generates its content.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Endpoint {
    /// Loop a static image, encoded as a continuous MPEG-TS stream.
    Stills {
        image_path: PathBuf,
        #[serde(default = "default_width")]
        width: u32,
        #[serde(default = "default_height")]
        height: u32,
    },
    /// Play a local media file into the crosspoint as a source.
    MediaPlayer {
        file_path: PathBuf,
        #[serde(default = "default_loop_playback")]
        loop_playback: bool,
        #[serde(default = "default_width")]
        width: u32,
        #[serde(default = "default_height")]
        height: u32,
    },
    /// Decode another registered source, rescale it, and re-encode the
    /// result as a new source — the one case here that isn't a pure
    /// relay/generator: it's a real transcode path.
    Scaler {
        source: String,
        #[serde(default = "default_width")]
        width: u32,
        #[serde(default = "default_height")]
        height: u32,
    },
}

impl Endpoint {
    fn needs_stdin(&self) -> bool {
        matches!(self, Endpoint::Scaler { .. })
    }
}

/// Exposed (not just used as a serde `default =`) so callers building an
/// `Endpoint` programmatically — `crates/router/src/management.rs`'s
/// `MediaEndpointRequest` conversion, in particular — can fall back to the
/// same values instead of duplicating the literals.
pub fn default_width() -> u32 {
    1280
}

pub fn default_height() -> u32 {
    720
}

pub fn default_loop_playback() -> bool {
    true
}

/// MPEG-TS packets are 188 bytes; ffmpeg's `pipe:1` is read and republished
/// in this-many-packets chunks (1316 bytes) rather than at arbitrary pipe-
/// read boundaries — the conventional SRT/MPEG-TS payload alignment, so a
/// downstream SRT output relays well-formed packet-aligned chunks.
const TS_PACKET_LEN: usize = 188;
const CHUNK_PACKETS: usize = 7;
const CHUNK_LEN: usize = TS_PACKET_LEN * CHUNK_PACKETS;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
enum MediaIoError {
    #[error("failed to spawn ffmpeg: {0}")]
    Spawn(#[source] std::io::Error),
}

fn ffmpeg_args(endpoint: &Endpoint) -> Vec<String> {
    let mut args: Vec<String> = vec!["-loglevel".into(), "error".into(), "-re".into()];
    let (width, height) = match endpoint {
        Endpoint::Stills {
            image_path,
            width,
            height,
        } => {
            args.extend([
                "-loop".into(),
                "1".into(),
                "-i".into(),
                image_path.display().to_string(),
            ]);
            (*width, *height)
        }
        Endpoint::MediaPlayer {
            file_path,
            loop_playback,
            width,
            height,
        } => {
            if *loop_playback {
                args.extend(["-stream_loop".into(), "-1".into()]);
            }
            args.extend(["-i".into(), file_path.display().to_string()]);
            (*width, *height)
        }
        Endpoint::Scaler {
            source: _,
            width,
            height,
        } => {
            args.extend(["-f".into(), "mpegts".into(), "-i".into(), "pipe:0".into()]);
            (*width, *height)
        }
    };
    args.extend([
        "-vf".into(),
        format!("scale={width}:{height}"),
        "-r".into(),
        "25".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-g".into(),
        "50".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "128k".into(),
        "-f".into(),
        "mpegts".into(),
        "pipe:1".into(),
    ]);
    args
}

async fn open(endpoint: &Endpoint) -> Result<Child, MediaIoError> {
    let stdin = if endpoint.needs_stdin() {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    Command::new("ffmpeg")
        .args(ffmpeg_args(endpoint))
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(MediaIoError::Spawn)
}

/// ffmpeg's own diagnostics (`-loglevel error`, so only real problems) are
/// worth keeping visible rather than silently discarding the pipe.
fn drain_stderr(stderr: ChildStderr, id: String) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            warn!(source = %id, "ffmpeg: {line}");
        }
    });
}

/// Spawn the task for one media-io source: run ffmpeg, publish every
/// MPEG-TS chunk it produces onto the crosspoint's broadcast channel for
/// `id`, and restart it if it ever exits. Runs until cancelled (or the
/// process exits).
pub fn spawn_input(
    id: String,
    endpoint: Endpoint,
    crosspoint: Arc<Crosspoint>,
) -> CancellationToken {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let tx = crosspoint.register_source(id.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = task_cancel.cancelled() => {
                    info!(source = %id, "media source stopped");
                    return;
                }
                result = open(&endpoint) => match result {
                    Ok(mut child) => {
                        // Scaler needs its upstream source's own Bytes fed
                        // into ffmpeg's stdin — subscribe before doing
                        // anything else so a missing/not-yet-registered
                        // upstream id is treated the same as a failed
                        // connect (retry after RECONNECT_DELAY), not a
                        // silent no-op.
                        let stdin_task = match (&endpoint, child.stdin.take()) {
                            (Endpoint::Scaler { source, .. }, Some(stdin)) => {
                                match crosspoint.subscribe(source) {
                                    Some(upstream_rx) => Some(tokio::spawn(feed_stdin(
                                        stdin,
                                        upstream_rx,
                                        task_cancel.clone(),
                                    ))),
                                    None => {
                                        let _ = child.kill().await;
                                        warn!(source = %id, upstream = %source, "scaler upstream source not found, retrying");
                                        None
                                    }
                                }
                            }
                            _ => None,
                        };
                        if endpoint.needs_stdin() && stdin_task.is_none() {
                            // Upstream lookup above already failed and
                            // killed the child; skip straight to the
                            // reconnect delay below.
                        } else {
                            info!(source = %id, "media source started");
                            if let Some(stderr) = child.stderr.take() {
                                drain_stderr(stderr, id.clone());
                            }
                            relay_stdout(&mut child, &tx, &id, &task_cancel).await;
                            if let Some(stdin_task) = stdin_task {
                                stdin_task.abort();
                            }
                            let _ = child.kill().await;
                            if task_cancel.is_cancelled() {
                                return;
                            }
                            warn!(source = %id, "media source ended, restarting");
                        }
                    }
                    Err(err) => {
                        warn!(source = %id, %err, "media source spawn failed, retrying");
                    }
                },
            }
            tokio::select! {
                _ = task_cancel.cancelled() => return,
                _ = sleep(RECONNECT_DELAY) => {}
            }
        }
    });
    cancel
}

/// Feeds one media-io source's own `Bytes` (the scaler's upstream) into
/// ffmpeg's stdin. Ends (dropping `stdin`, which gives ffmpeg a clean EOF)
/// when cancelled or when the upstream source goes away.
async fn feed_stdin(
    mut stdin: ChildStdin,
    mut upstream_rx: broadcast::Receiver<Bytes>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            frame = upstream_rx.recv() => match frame {
                Ok(bytes) => {
                    if stdin.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

async fn relay_stdout(
    child: &mut Child,
    tx: &broadcast::Sender<Bytes>,
    id: &str,
    cancel: &CancellationToken,
) {
    let Some(mut stdout) = child.stdout.take() else {
        return;
    };
    let mut buf = BytesMut::with_capacity(CHUNK_LEN * 4);
    let mut read_buf = [0u8; 4096];
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            result = stdout.read(&mut read_buf) => match result {
                Ok(0) => return,
                Ok(n) => {
                    buf.extend_from_slice(&read_buf[..n]);
                    while buf.len() >= CHUNK_LEN {
                        let chunk = buf.split_to(CHUNK_LEN).freeze();
                        // No receivers is normal (no output currently
                        // routed here) — send() erroring in that case
                        // isn't a problem.
                        let _ = tx.send(chunk);
                    }
                }
                Err(err) => {
                    warn!(source = %id, %err, "media source read error");
                    return;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rename_all = "lowercase"` on a `MediaPlayer` variant produces
    /// `"mediaplayer"` (no separator) — easy to get wrong by assuming
    /// `"media-player"` or `"media_player"` when wiring up a client (the
    /// web UI's JS, `management.rs`'s `MediaEndpointRequest`) against this
    /// wire format. Pins the exact strings so that assumption gets caught
    /// here instead of as a silent 422 from the REST API.
    #[test]
    fn mode_tag_values_have_no_separator() {
        let stills: Endpoint =
            serde_json::from_str(r#"{"mode":"stills","image_path":"/tmp/x.png"}"#).unwrap();
        assert!(matches!(stills, Endpoint::Stills { .. }));

        let player: Endpoint =
            serde_json::from_str(r#"{"mode":"mediaplayer","file_path":"/tmp/x.mp4"}"#).unwrap();
        assert!(matches!(player, Endpoint::MediaPlayer { .. }));

        let scaler: Endpoint = serde_json::from_str(r#"{"mode":"scaler","source":"a"}"#).unwrap();
        assert!(matches!(scaler, Endpoint::Scaler { .. }));
    }
}
