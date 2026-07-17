//! Integration test that exercises the full input -> crosspoint -> output
//! relay path over a *real* OMT connection: the test acts as both the
//! "camera" (a raw `sys::omt_send_create` sender) and the "monitor" (a raw
//! `sys::omt_receive_create` receiver), with `omt-io`'s own
//! `spawn_input`/`spawn_output` in between doing the actual relay — the
//! same encoder/decoder-around-the-router shape `crates/ndi-io/tests` and
//! `crates/srt-io/tests` use for their transports. Requires the real OMT
//! SDK (`OMT_LIB_DIR`, see `build.rs`); not run in CI.
//!
//! Lessons learned from `ndi-io`'s equivalent test are applied from the
//! start here rather than re-discovered: a dedicated continuous-framerate
//! sender thread (not opportunistic sends from inside a polling loop), and
//! `rt.shutdown_background()` instead of letting `tokio::runtime::Runtime`
//! drop naturally (which blocks on this crate's intentionally-forever
//! `spawn_blocking` tasks).

use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crosspoint_core::Crosspoint;
use omt_io::sys::{
    self, OMTFrameTypeMask, OMTMediaFrame, OMTPreferredVideoFormat, OMTQuality, OMTReceiveFlags,
};
use omt_io::{spawn_input, spawn_output, Endpoint};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);
const FRAME_TYPE_VIDEO: i32 = 2;
const CODEC_UYVY: i32 = 0x5956_5955;

fn uyvy_frame(width: i32, height: i32, data: &mut Vec<u8>) -> OMTMediaFrame {
    let stride = width * 2;
    data.resize((stride * height) as usize, 0x80);
    OMTMediaFrame {
        frame_type: FRAME_TYPE_VIDEO,
        timestamp: -1, // let the SDK generate timestamps
        codec: CODEC_UYVY,
        width,
        height,
        stride,
        flags: 0,
        frame_rate_n: 30,
        frame_rate_d: 1,
        aspect_ratio: width as f32 / height as f32,
        color_space: 0,
        sample_rate: 0,
        channels: 0,
        samples_per_channel: 0,
        data: data.as_mut_ptr() as *mut c_void,
        data_length: data.len() as i32,
        ..Default::default()
    }
}

#[test]
fn relays_a_video_frame_end_to_end_over_real_omt() {
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
                address: "omt-io-test-camera".into(),
            },
            crosspoint.clone(),
        );
        spawn_output(
            "program".into(),
            Endpoint::Sender {
                name: "omt-io-test-output".into(),
            },
            "cam1".into(),
            crosspoint.clone(),
        );
    });

    // The test's own "camera": a raw OMT sender the router's input should
    // discover and receive from. Runs on its own thread at a steady ~30fps
    // for the whole test — a real camera free-running, not an
    // opportunistic send from inside a discovery-polling loop (that
    // specific shortcut looked plausible but wasn't nearly frequent enough
    // to look like a live connection when this was first tried for NDI).
    let stop = Arc::new(AtomicBool::new(false));
    let camera_thread = {
        let stop = stop.clone();
        thread::spawn(move || {
            let name = CString::new("omt-io-test-camera").unwrap();
            let sender = unsafe { sys::omt_send_create(name.as_ptr(), OMTQuality::Default) };
            assert!(!sender.is_null(), "omt_send_create failed for test camera");
            let mut buf = Vec::new();
            let mut frame = uyvy_frame(64, 64, &mut buf);
            while !stop.load(Ordering::Relaxed) {
                unsafe { sys::omt_send(sender, &mut frame) };
                thread::sleep(Duration::from_millis(33));
            }
            unsafe { sys::omt_send_destroy(sender) };
        })
    };

    // The test's own "monitor": a raw OMT receiver watching the router's
    // output.
    eprintln!("[test] discovering router output...");
    let monitor_address = {
        let start = Instant::now();
        loop {
            let addrs = omt_test_support::list_addresses();
            if let Some(a) = addrs.iter().find(|a| a.contains("omt-io-test-output")) {
                break a.clone();
            }
            assert!(
                start.elapsed() < DISCOVERY_TIMEOUT,
                "timed out waiting to discover the router's OMT output"
            );
            thread::sleep(Duration::from_millis(500));
        }
    };
    eprintln!("[test] found router output, connecting monitor...");

    let monitor_address_c = CString::new(monitor_address).unwrap();
    let monitor = unsafe {
        sys::omt_receive_create(
            monitor_address_c.as_ptr(),
            OMTFrameTypeMask::VIDEO,
            OMTPreferredVideoFormat::UYVY,
            OMTReceiveFlags::None,
        )
    };
    assert!(!monitor.is_null(), "omt_receive_create failed for monitor");

    eprintln!("[test] monitor connected, waiting for relayed frame...");
    let received = {
        let start = Instant::now();
        loop {
            let frame_ptr = unsafe { sys::omt_receive(monitor, OMTFrameTypeMask::VIDEO, 200) };
            if !frame_ptr.is_null() {
                let frame = unsafe { *frame_ptr };
                if frame.frame_type == FRAME_TYPE_VIDEO {
                    break frame;
                }
            }
            assert!(
                start.elapsed() < CAPTURE_TIMEOUT,
                "timed out waiting for the relayed frame"
            );
        }
    };
    eprintln!("[test] relayed frame received!");

    unsafe { sys::omt_receive_destroy(monitor) };
    stop.store(true, Ordering::Relaxed);
    camera_thread.join().expect("camera thread panicked");
    rt.shutdown_background();

    assert_eq!(received.width, 64);
    assert_eq!(received.height, 64);
    assert_eq!(received.codec, CODEC_UYVY);
    assert_eq!(received.data_length, 64 * 2 * 64);
}

/// Tiny discovery helper duplicated from `omt_io`'s own (private)
/// `list_addresses` — kept minimal and test-local rather than exposing
/// another crate internal just for this.
mod omt_test_support {
    use omt_io::sys;
    use std::ffi::CStr;

    pub fn list_addresses() -> Vec<String> {
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
}
