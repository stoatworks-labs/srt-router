//! Integration tests that exercise the full input -> crosspoint -> output
//! relay path over *real* SRT connections (real handshake, real protocol
//! traffic via srt-tokio acting as the test's "encoder"/"decoder" clients)
//! rather than just asserting a UDP socket is bound. This is the strongest
//! local verification available without real third-party SRT hardware.

use std::time::{Duration, Instant};

use bytes::Bytes;
use crosspoint_core::Crosspoint;
use futures::SinkExt;
use srt_io::{spawn_input, spawn_output, Endpoint};
use srt_tokio::SrtSocket;
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;

const RECV_TIMEOUT: Duration = Duration::from_secs(5);
/// Buffer for connection setup / task scheduling on localhost before we
/// start relying on ordering between "peer connected" and "relay task
/// subscribed to its source" — see the module doc on why this matters.
const SETTLE: Duration = Duration::from_millis(400);

async fn call(addr: &str) -> SrtSocket {
    SrtSocket::builder()
        .call(addr, None)
        .await
        .unwrap_or_else(|e| panic!("failed to connect to {addr}: {e}"))
}

#[tokio::test]
async fn relays_bytes_end_to_end_over_real_srt() {
    let crosspoint = Crosspoint::new();

    spawn_input(
        "test-source".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18601".into(),
        },
        crosspoint.clone(),
    );
    spawn_output(
        "test-output".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18602".into(),
        },
        "test-source".into(),
        crosspoint.clone(),
    );
    sleep(SETTLE).await;

    // Real SRT clients standing in for an encoder and a decoder, exactly as
    // a real one would connect to this router.
    let mut encoder = call("127.0.0.1:18601").await;
    let mut decoder = call("127.0.0.1:18602").await;
    sleep(SETTLE).await;

    let payload = Bytes::from_static(b"hello from a real srt sender");
    encoder
        .send((Instant::now(), payload.clone()))
        .await
        .expect("encoder send");

    let (_t, received) = timeout(RECV_TIMEOUT, decoder.try_next())
        .await
        .expect("timed out waiting for the relayed payload")
        .expect("decoder read error")
        .expect("decoder stream ended before receiving anything");

    assert_eq!(received, payload);
}

#[tokio::test]
async fn output_switches_source_live_over_real_srt() {
    let crosspoint = Crosspoint::new();

    spawn_input(
        "source-a".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18611".into(),
        },
        crosspoint.clone(),
    );
    spawn_input(
        "source-b".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18612".into(),
        },
        crosspoint.clone(),
    );
    spawn_output(
        "test-output".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18613".into(),
        },
        "source-a".into(),
        crosspoint.clone(),
    );
    sleep(SETTLE).await;

    let mut encoder_a = call("127.0.0.1:18611").await;
    let mut encoder_b = call("127.0.0.1:18612").await;
    let mut decoder = call("127.0.0.1:18613").await;
    sleep(SETTLE).await;

    // Routed to source-a by default: a payload from A arrives, one from B
    // (sent but never selected) does not.
    encoder_a
        .send((Instant::now(), Bytes::from_static(b"from-a-1")))
        .await
        .unwrap();
    encoder_b
        .send((Instant::now(), Bytes::from_static(b"from-b-ignored")))
        .await
        .unwrap();

    let (_t, first) = timeout(RECV_TIMEOUT, decoder.try_next())
        .await
        .expect("timed out waiting for first payload")
        .expect("decoder read error")
        .expect("decoder stream ended early");
    assert_eq!(first, Bytes::from_static(b"from-a-1"));

    // Live re-route to source-b, same output connection, no reconnect.
    assert!(crosspoint.route("test-output", "source-b"));
    sleep(SETTLE).await;

    encoder_b
        .send((Instant::now(), Bytes::from_static(b"from-b-1")))
        .await
        .unwrap();

    let (_t, second) = timeout(RECV_TIMEOUT, decoder.try_next())
        .await
        .expect("timed out waiting for second payload")
        .expect("decoder read error")
        .expect("decoder stream ended early");
    assert_eq!(second, Bytes::from_static(b"from-b-1"));
}

/// Removing a SOURCE must not disturb an output that happens to be routed to
/// it. The output's own connection is not what changed.
///
/// This used to tear the decoder off: `relay_out` treated the source's
/// broadcast channel closing as a socket failure and returned, so
/// `spawn_output` dropped the SRT socket, logged "SRT output disconnected,
/// reconnecting", slept 2 s and re-opened. A listener-mode output kicked the
/// connected decoder; a caller-mode one showed the far end a disconnect. All
/// from deleting a different entity.
///
/// `Crosspoint::deregister_source` documents the opposite: an output's
/// `subscribe` "will start returning `None`/closed — the caller ... is expected
/// to treat that the same as 'nothing routed yet' until re-routed."
#[tokio::test]
async fn removing_a_source_does_not_disconnect_outputs_routed_to_it() {
    let crosspoint = Crosspoint::new();

    let doomed_input = spawn_input(
        "doomed".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18621".into(),
        },
        crosspoint.clone(),
    );
    spawn_input(
        "survivor".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18622".into(),
        },
        crosspoint.clone(),
    );
    spawn_output(
        "program".into(),
        Endpoint::Listener {
            bind: "127.0.0.1:18623".into(),
        },
        "doomed".into(),
        crosspoint.clone(),
    );
    sleep(SETTLE).await;

    let mut doomed = call("127.0.0.1:18621").await;
    let mut survivor = call("127.0.0.1:18622").await;
    let mut decoder = call("127.0.0.1:18623").await;
    sleep(SETTLE).await;

    doomed
        .send((Instant::now(), Bytes::from_static(b"before")))
        .await
        .unwrap();
    let (_t, first) = timeout(RECV_TIMEOUT, decoder.try_next())
        .await
        .expect("timed out waiting for the first payload")
        .expect("decoder read error")
        .expect("decoder stream ended early");
    assert_eq!(first, Bytes::from_static(b"before"));

    // Delete the source the output is routed from — which is what the
    // management API does: cancel its task AND deregister it. Cancelling
    // matters, because the input task owns the broadcast Sender; removing the
    // entry from the map alone leaves the channel open and the output never
    // sees Closed at all.
    drop(doomed);
    doomed_input.cancel();
    assert!(crosspoint.deregister_source("doomed"));
    sleep(SETTLE).await;

    // Re-route to a source that is still there. If the output had been torn
    // down and put back, this decoder's connection would be gone and the read
    // below would fail rather than deliver.
    assert!(crosspoint.route("program", "survivor"));
    sleep(SETTLE).await;

    survivor
        .send((Instant::now(), Bytes::from_static(b"after")))
        .await
        .unwrap();

    let (_t, second) = timeout(RECV_TIMEOUT, decoder.try_next())
        .await
        .expect("the decoder was disconnected by an unrelated source removal")
        .expect("decoder read error")
        .expect("decoder stream ended early");
    assert_eq!(second, Bytes::from_static(b"after"));
}
