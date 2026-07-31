//! Integration test that exercises the full input -> crosspoint -> output
//! relay path over a *real* NDI connection: the test acts as both the
//! "camera" (a real `ndi_io::sys::Sender`) and the "monitor" (a real
//! `ndi_io::sys::Receiver`), with `ndi-io`'s own `spawn_input`/`spawn_output`
//! in between doing the actual relay — exactly the encoder/decoder-around-
//! the-router shape `crates/srt-io/tests/relay.rs` uses for SRT.
//!
//! Needs a real NDI *runtime* but no SDK to build (see `src/sys.rs`); skips
//! itself when none is installed, so it is safe to run anywhere.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crosspoint_core::Crosspoint;
use ndi_io::sys::{self, Finder, Frame, Receiver, Sender, VideoFrame};
use ndi_io::{spawn_input, spawn_output, Endpoint};

/// BGRX is what the receiver's colour format (`BGRX_BGRA`) yields for a source
/// with no alpha, so it is both what the camera sends and what the monitor
/// should see coming back out of the router.
const FOURCC_BGRX: u32 = u32::from_le_bytes(*b"BGRX");

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn relays_a_video_frame_end_to_end_over_real_ndi() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let Ok(api) = sys::load() else {
        eprintln!("no NDI runtime installed — skipping");
        return;
    };
    let rt = tokio::runtime::Runtime::new().expect("build tokio runtime");
    let crosspoint = Crosspoint::new();

    rt.block_on(async {
        spawn_input(
            "cam1".into(),
            Endpoint::Receiver {
                source_name: "ndi-io-test-camera".into(),
            },
            crosspoint.clone(),
        );
        spawn_output(
            "program".into(),
            Endpoint::Sender {
                name: "ndi-io-test-output".into(),
            },
            "cam1".into(),
            crosspoint.clone(),
        );
    });

    // The test's own "camera": a plain NDI sender the router's input should
    // discover and capture from. Runs on its own thread at a steady ~30fps
    // for the whole test, like a real camera free-running — not just
    // sending opportunistically from inside the discovery-polling loops
    // below, which don't run anywhere near fast/regularly enough to look
    // like a live connection to the receiver on the other end.
    let stop = Arc::new(AtomicBool::new(false));
    let camera_thread = {
        let stop = stop.clone();
        let api = api.clone();
        thread::spawn(move || {
            let mut camera =
                Sender::new(api, "ndi-io-test-camera").expect("create test camera sender");
            // A recognisable, non-uniform payload: a uniform frame would pass
            // even if the relay dropped rows.
            let stride: i32 = 64 * 4;
            let data = (0..(stride * 64)).map(|i| (i % 251) as u8).collect();
            let frame = VideoFrame {
                xres: 64,
                yres: 64,
                four_cc: FOURCC_BGRX,
                frame_rate_n: 30,
                frame_rate_d: 1,
                picture_aspect_ratio: 0.0,
                frame_format_type: 1, // progressive
                timecode: sys::SEND_TIMECODE_SYNTHESIZE,
                line_stride_in_bytes: stride,
                data,
            };
            while !stop.load(Ordering::Relaxed) {
                camera.send_video(&frame);
                thread::sleep(Duration::from_millis(33));
            }
        })
    };

    // The test's own "monitor": a plain NDI receiver watching the router's
    // output.
    let mut finder = Finder::new(api.clone(), true).expect("create finder");
    eprintln!("[test] discovering router output...");
    let monitor_source = {
        let start = Instant::now();
        loop {
            if let Some(s) = finder
                .current_sources()
                .into_iter()
                .find(|s| s.name.contains("ndi-io-test-output"))
            {
                break s;
            }
            assert!(
                start.elapsed() < DISCOVERY_TIMEOUT,
                "timed out waiting to discover the router's NDI output"
            );
            finder.wait_for_sources(Duration::from_millis(500));
        }
    };
    eprintln!("[test] found router output, creating monitor receiver...");

    let mut monitor = Receiver::connect(api, &monitor_source, "ndi-io-test-monitor")
        .expect("create test monitor receiver");

    eprintln!("[test] monitor receiver created, waiting for relayed frame...");
    let received = {
        let start = Instant::now();
        loop {
            // Audio and metadata can arrive first; only a video frame ends this.
            if let Some(Frame::Video(f)) = monitor.capture(Duration::from_millis(200)) {
                break f;
            }
            assert!(
                start.elapsed() < CAPTURE_TIMEOUT,
                "timed out waiting for the relayed frame"
            );
        }
    };
    eprintln!("[test] relayed frame received!");

    stop.store(true, Ordering::Relaxed);
    camera_thread.join().expect("camera thread panicked");

    // spawn_input/spawn_output's blocking tasks loop forever by design (see
    // their doc comments) — a real router process exits the whole process
    // when it's done, but a #[test] fn returning just drops `rt`, and
    // `Runtime::drop` blocks until every outstanding `spawn_blocking` task
    // finishes, which these never do. `shutdown_background` drops it
    // without waiting, which is what we actually want here.
    rt.shutdown_background();

    assert_eq!(received.xres, 64);
    assert_eq!(received.yres, 64);
    assert_eq!(
        received.four_cc, FOURCC_BGRX,
        "expected BGRX out of the router"
    );
    assert_eq!(received.data.len(), 64 * 64 * 4);
    // The payload survived the crosspoint, not just the frame header. This is
    // what the old envelope could not guarantee for a padded stride.
    assert_eq!(received.line_stride_in_bytes, 64 * 4);
    assert!(
        received.data.iter().any(|&b| b != received.data[0]),
        "relayed frame is uniform — the payload did not survive"
    );
}
