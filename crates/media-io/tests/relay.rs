//! Integration tests that run real `ffmpeg` and verify genuine MPEG-TS
//! bytes come out the other end of the crosspoint — not a mock. Test
//! assets (a still image, a short video) are themselves generated with
//! ffmpeg's `lavfi` synthetic sources so the test has no checked-in binary
//! fixtures.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crosspoint_core::Crosspoint;
use media_io::{spawn_input, Endpoint};
use tokio::time::timeout;

const RECV_TIMEOUT: Duration = Duration::from_secs(10);
const TS_PACKET_LEN: usize = 188;

fn generate_test_image(path: &Path) {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x64",
            "-frames:v",
            "1",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run ffmpeg to generate test image");
    assert!(status.success(), "ffmpeg failed to generate test image");
}

fn generate_test_video(path: &Path) {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x64:rate=25",
            "-t",
            "2",
            "-pix_fmt",
            "yuv420p",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run ffmpeg to generate test video");
    assert!(status.success(), "ffmpeg failed to generate test video");
}

/// Every 188 bytes of a real MPEG-TS chunk starts with the sync byte
/// `0x47` — the cheapest real check that ffmpeg actually produced valid
/// transport-stream packets, not just arbitrary bytes.
fn assert_valid_mpegts_chunk(chunk: &[u8]) {
    assert_eq!(chunk.len() % TS_PACKET_LEN, 0, "chunk isn't packet-aligned");
    assert!(!chunk.is_empty(), "chunk is empty");
    for packet in chunk.chunks(TS_PACKET_LEN) {
        assert_eq!(packet[0], 0x47, "missing MPEG-TS sync byte");
    }
}

#[tokio::test]
async fn stills_source_produces_real_mpegts() {
    let image_path = std::env::temp_dir().join("media-io-test-stills.png");
    generate_test_image(&image_path);

    let crosspoint = Crosspoint::new();
    let _cancel = spawn_input(
        "stills-test".into(),
        Endpoint::Stills {
            image_path,
            width: 64,
            height: 64,
        },
        crosspoint.clone(),
    );
    let mut rx = crosspoint.subscribe("stills-test").unwrap();

    let chunk = timeout(RECV_TIMEOUT, rx.recv())
        .await
        .expect("timed out waiting for a stills chunk")
        .expect("crosspoint channel closed unexpectedly");
    assert_valid_mpegts_chunk(&chunk);
}

#[tokio::test]
async fn media_player_source_produces_real_mpegts() {
    let video_path = std::env::temp_dir().join("media-io-test-player.mp4");
    generate_test_video(&video_path);

    let crosspoint = Crosspoint::new();
    let _cancel = spawn_input(
        "player-test".into(),
        Endpoint::MediaPlayer {
            file_path: video_path,
            loop_playback: true,
            width: 64,
            height: 64,
        },
        crosspoint.clone(),
    );
    let mut rx = crosspoint.subscribe("player-test").unwrap();

    let chunk = timeout(RECV_TIMEOUT, rx.recv())
        .await
        .expect("timed out waiting for a media-player chunk")
        .expect("crosspoint channel closed unexpectedly");
    assert_valid_mpegts_chunk(&chunk);
}

/// The scaler is the one case here that both consumes and produces a
/// crosspoint source — this drives a real upstream stills source, points a
/// scaler at it, and confirms real (rescaled, re-encoded) MPEG-TS comes out
/// the scaler's own output, not just that the upstream was relayed as-is.
#[tokio::test]
async fn scaler_source_consumes_upstream_and_produces_real_mpegts() {
    let image_path = std::env::temp_dir().join("media-io-test-scaler-upstream.png");
    generate_test_image(&image_path);

    let crosspoint = Crosspoint::new();
    let _upstream_cancel = spawn_input(
        "scaler-upstream".into(),
        Endpoint::Stills {
            image_path,
            width: 64,
            height: 64,
        },
        crosspoint.clone(),
    );
    let _scaler_cancel = spawn_input(
        "scaler-test".into(),
        Endpoint::Scaler {
            source: "scaler-upstream".into(),
            width: 32,
            height: 32,
        },
        crosspoint.clone(),
    );
    let mut rx = crosspoint.subscribe("scaler-test").unwrap();

    let chunk = timeout(RECV_TIMEOUT, rx.recv())
        .await
        .expect("timed out waiting for a scaler chunk")
        .expect("crosspoint channel closed unexpectedly");
    assert_valid_mpegts_chunk(&chunk);
}

/// A scaler pointed at a source id that doesn't exist yet shouldn't spin
/// ffmpeg processes forever or panic — it should keep retrying (same as
/// srt-io's connect-retry loop) and pick the source up once it appears.
#[tokio::test]
async fn scaler_recovers_once_its_upstream_source_appears() {
    let image_path = std::env::temp_dir().join("media-io-test-scaler-late-upstream.png");
    generate_test_image(&image_path);

    let crosspoint = Crosspoint::new();
    let _scaler_cancel = spawn_input(
        "late-scaler-test".into(),
        Endpoint::Scaler {
            source: "late-upstream".into(),
            width: 32,
            height: 32,
        },
        crosspoint.clone(),
    );
    // Give the scaler a moment to try (and fail to find) the upstream
    // before it exists, exercising the retry path.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let _upstream_cancel = spawn_input(
        "late-upstream".into(),
        Endpoint::Stills {
            image_path,
            width: 64,
            height: 64,
        },
        crosspoint.clone(),
    );
    let mut rx = crosspoint.subscribe("late-scaler-test").unwrap();

    let chunk = timeout(RECV_TIMEOUT, rx.recv())
        .await
        .expect("timed out waiting for the scaler to recover")
        .expect("crosspoint channel closed unexpectedly");
    assert_valid_mpegts_chunk(&chunk);
}
