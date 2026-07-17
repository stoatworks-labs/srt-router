# Roadmap

## Phase 1 — relay crosspoint + web UI (current)

- [x] Transport-agnostic crosspoint engine (`crates/core`), unit tested.
- [x] SRT relay input/output (listener + caller, auto-reconnect)
      (`crates/srt-io`).
- [x] Local web UI: crosspoint grid, click-to-route, polling REST API
      (`crates/web`).
- [x] TOML-configured router binary (`crates/router`).
- [x] Verified locally: unit tests, real bound SRT/UDP listener sockets,
      live route changes via both the REST API and an actual browser click.
- [x] Integration tests that relay real SRT protocol traffic end-to-end
      through the crosspoint (`crates/srt-io/tests/relay.rs`), using
      `srt-tokio` clients as the encoder/decoder — including a live
      re-route mid-stream over an already-established connection. This is
      meaningfully stronger than the loopback/`lsof`-level checks above,
      though it's still this router's own SRT stack talking to itself, not
      third-party hardware.
- [x] CI (GitHub Actions): `fmt --check`, `clippy -D warnings`, `cargo
      test` on every push/PR.
- [x] Runtime add/remove: `POST`/`DELETE /api/manage/sources` and
      `/outputs` (`crates/router/src/management.rs`), backed by a
      `CancellationToken` per task (added to `srt-io`) so removal actually
      stops the task and frees the socket — confirmed with `lsof`, not
      assumed. Config-loaded and API-added sources/outputs go through the
      same code path (`crates/router/src/registry.rs`), so one isn't a
      second-class citizen relative to the other. Web UI: Add
      source/destination forms + a remove control per row/column, verified
      live in a real browser (add cam3, confirm its port binds, remove it,
      confirm the port frees).
- [ ] Verified against a real **third-party** SRT encoder/decoder or over a
      real (non-loopback) network path — this is the one remaining gap
      before treating Phase 1 as production-ready.

## Transports beyond SRT

- [x] **NDI** — `crates/ndi-io`, using
      [grafton-ndi](https://github.com/GrantSparks/grafton-ndi)
      (Apache-2.0) against the real NDI SDK. Same `spawn_input`/
      `spawn_output` shape as `srt-io`, with a small envelope
      (`src/envelope.rs`) carrying NDI's video/audio/metadata frames
      through `crosspoint-core`'s existing `Bytes` channel — see
      [architecture.md](architecture.md#this-isnt-hypothetical--cratesndi-io-proves-it).
      Verified for real: `crates/ndi-io/tests/relay.rs` drives an actual
      NDI sender and receiver against it, consistently passing.
      **Config-only for now**: `crates/router` gained an optional `ndi`
      Cargo feature and `config::Transport` (an untagged enum picking
      `srt_io::Endpoint` vs `ndi_io::Endpoint` by their disjoint `mode`
      values, so existing SRT-only TOML files need no changes) — build
      with `cargo run --features ndi` and add `mode = "receiver"` /
      `"sender"` inputs/outputs to use it. `management.rs`'s runtime
      add-source/add-destination API is still SRT-only, so the web UI
      still shows NDI as a disabled dropdown option — that's the next
      remaining gap, not the config path. Also excluded from default CI
      (needs the real SDK
      installed, which CI can't do).
- [x] **Cross-kind route validation** — now that SRT and NDI can genuinely
      coexist in one config, `POST /api/route` rejects routing a source to
      an output of a different kind (e.g. an NDI source into an SRT
      output) with a clear error, instead of silently accepting a route
      that would relay one transport's envelope into a socket expecting
      another's. `crosspoint-web::app_with_kind_lookup` takes a
      `KindLookup` closure (`crates/router` wires it to
      `Registry::kind_of`) — `crosspoint-core` itself still knows nothing
      about kinds, per its design. Verified two ways: unit tests in
      `crates/web/src/lib.rs` (same-kind allowed, cross-kind rejected with
      the route left untouched, and the no-guard case behaves exactly as
      before), and live against a real mixed SRT+NDI config
      (`cargo run --features ndi`) via `curl`.
- [ ] **OMT** — `crates/omt-io` exists only as a placeholder. OMT itself is
      a genuinely open, MIT-licensed protocol (unlike NDI), but the only
      existing Rust wrapper is Windows-only and pre-release (no
      send/receive implementation, only source discovery). Real support
      means hand-writing FFI bindings against libomt's own C header and
      the prebuilt macOS/Windows binary (no Linux build exists) —
      deferred, not started.

## Phase 2 — special-purpose sources

The core engine's `Source` abstraction (see
[architecture.md](architecture.md)) is meant to support these without
changing `crates/core` — each is a new producer that calls
`Crosspoint::register_source` and publishes chunks it generated instead of
relayed. The web UI's Add source/destination forms already list these as
disabled options ("Phase 2 — not built yet") so the menu shape exists
ahead of the backends:

- **Stills source** — loop a static image, encoded as an SRT-compatible
  stream (likely via an `ffmpeg`/`gstreamer` child process or library
  binding — needs a real decision on which, see below).
- **Media player source** — play a local file (loop or one-shot) into the
  crosspoint as a source.
- **Scaler source** — take another registered source, decode, rescale,
  re-encode, and publish the result as a new source (the one case where
  "pure relay" isn't the point — this is deliberately a real transcode
  path).

Open question, not yet decided: whether these are built on `ffmpeg`/
`gstreamer` as external processes (simpler, well-tested codecs, but a
runtime dependency and process-management overhead) or Rust-native
encode/decode crates (no external dependency, but more integration work and
narrower codec support). Needs research before starting Phase 2.

## Phase 3 — operational hardening

- [x] Persist crosspoint routing across a restart — optional `[state]` in
      the TOML config, output -> source routes written to a JSON file
      write-then-rename on change, reloaded at startup (overriding each
      output's `default_source`). Off by default (in-memory only, as
      before) unless `[state]` is configured. `crates/router/src/state.rs`.
- [x] Websocket push for the web UI (`GET /ws`) instead of polling alone —
      pushes on connect and on every state change (~200ms detection
      latency), grid updates with zero client-side polling once connected.
      REST `GET /api/state` kept for first paint and as a fallback if the
      upgrade is ever blocked. `crates/web/src/lib.rs`.
- [ ] Auth (at least a shared secret) and optionally TLS on the web UI/API —
      currently assumes a trusted operations network, same as most hardware
      routers' control ports, but that assumption should be explicit and
      optionally removable. Not started — needs a decision on the auth
      model (shared secret vs. something richer) before implementing.
- [ ] Surface SRT connection health (the socket statistics `srt-tokio`/SRT
      itself exposes — RTT, loss, bitrate) in the web UI and API, not just
      connected/disconnected.
- [ ] Multiview: thumbnail/preview of each source in the web UI grid, not
      just its id.

## Phase 4 — external control

- A control API stable enough for other software to drive routing — e.g.
  eventually tying this into the [AV Mainframe](../../av-mainframe) project,
  or Bitfocus Companion for physical-panel control. Deferred until the
  REST API's shape has settled from real use, so it isn't locked in
  prematurely.
