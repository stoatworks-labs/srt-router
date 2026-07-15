//! Integration test that exercises the full input -> crosspoint -> output
//! relay path over a *real* NDI connection: the test acts as both the
//! "camera" (a real `grafton_ndi::Sender`) and the "monitor" (a real
//! `grafton_ndi::Receiver`), with `ndi-io`'s own `spawn_input`/`spawn_output`
//! in between doing the actual relay — exactly the encoder/decoder-around-
//! the-router shape `crates/srt-io/tests/relay.rs` uses for SRT. Requires
//! the real NDI SDK/runtime (see the workspace README); not run in CI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crosspoint_core::Crosspoint;
use grafton_ndi::{
    Finder, FinderOptions, PixelFormat, Receiver, ReceiverOptions, Sender, SenderOptions,
    VideoFrame, NDI,
};
use ndi_io::{spawn_input, spawn_output, Endpoint};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn relays_a_video_frame_end_to_end_over_real_ndi() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
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
        thread::spawn(move || {
            let ndi = NDI::new().expect("NDI init (camera thread)");
            let camera = Sender::new(&ndi, &SenderOptions::builder("ndi-io-test-camera").build())
                .expect("create test camera sender");
            let frame = VideoFrame::builder()
                .resolution(64, 64)
                .pixel_format(PixelFormat::BGRX)
                .frame_rate(30, 1)
                .build()
                .expect("build test frame");
            while !stop.load(Ordering::Relaxed) {
                camera.send_video(&frame);
                thread::sleep(Duration::from_millis(33));
            }
        })
    };

    let ndi = NDI::new().expect("NDI init (test thread)");

    // The test's own "monitor": a plain NDI receiver watching the router's
    // output.
    let finder = Finder::new(
        &ndi,
        &FinderOptions::builder().show_local_sources(true).build(),
    )
    .expect("create finder");
    eprintln!("[test] discovering router output...");
    let monitor_source = {
        let start = Instant::now();
        loop {
            let sources = finder.current_sources().expect("list sources");
            if let Some(s) = sources
                .iter()
                .find(|s| s.name.contains("ndi-io-test-output"))
            {
                break s.clone();
            }
            assert!(
                start.elapsed() < DISCOVERY_TIMEOUT,
                "timed out waiting to discover the router's NDI output"
            );
            finder.wait_for_sources(Duration::from_millis(500)).ok();
        }
    };
    eprintln!("[test] found router output, creating monitor receiver...");

    let monitor = Receiver::new(&ndi, &ReceiverOptions::builder(monitor_source).build())
        .expect("create test monitor receiver");

    eprintln!("[test] monitor receiver created, waiting for relayed frame...");
    let received = {
        let start = Instant::now();
        loop {
            if let Some(f) = monitor
                .video()
                .try_capture(Duration::from_millis(200))
                .expect("capture video")
            {
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

    assert_eq!(received.width(), 64);
    assert_eq!(received.height(), 64);
    assert_eq!(received.pixel_format(), PixelFormat::BGRX);
    assert_eq!(received.data().len(), 64 * 64 * 4);
}
