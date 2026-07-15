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
- [ ] Verified against a real **third-party** SRT encoder/decoder or over a
      real (non-loopback) network path — this is the one remaining gap
      before treating Phase 1 as production-ready.

## Phase 2 — special-purpose sources

The core engine's `Source` abstraction (see
[architecture.md](architecture.md)) is meant to support these without
changing `crates/core` — each is a new producer that calls
`Crosspoint::register_source` and publishes chunks it generated instead of
relayed:

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
