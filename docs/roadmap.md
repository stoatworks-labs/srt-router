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
- [ ] Verified against a real SRT encoder/decoder or over a real network
      path — only local/loopback-adjacent testing has happened so far. This
      is the main open gap before treating Phase 1 as production-ready.

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

- Persist crosspoint routing across a restart (currently in-memory only —
  a restart resets every output to its config `default_source`).
- Auth (at least a shared secret) and optionally TLS on the web UI/API —
  currently assumes a trusted operations network, same as most hardware
  routers' control ports, but that assumption should be explicit and
  optionally removable.
- Websocket push for the web UI instead of 1s polling, for a snappier grid
  and to stop hammering `/api/state` when idle.
- Surface SRT connection health (the socket statistics `srt-tokio`/SRT
  itself exposes — RTT, loss, bitrate) in the web UI and API, not just
  connected/disconnected.
- Multiview: thumbnail/preview of each source in the web UI grid, not just
  its id.

## Phase 4 — external control

- A control API stable enough for other software to drive routing — e.g.
  eventually tying this into the [AV Mainframe](../../av-mainframe) project,
  or Bitfocus Companion for physical-panel control. Deferred until the
  REST API's shape has settled from real use, so it isn't locked in
  prematurely.
