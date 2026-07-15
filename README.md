# srt-router

> **AI-assisted project.** This codebase was created with [Claude](https://claude.com/claude-code)
> (Anthropic), directed and reviewed by a human author. The relay/crosspoint
> engine and web UI have been exercised locally — including integration
> tests that relay real SRT protocol traffic end-to-end and a live
> crosspoint switch via the web UI, see [Status](#status) — but **not yet
> run against real-world third-party SRT encoders/decoders or over an
> actual (non-loopback) network path**. Review before relying on it for
> anything live.

A crosspoint-based [SRT](https://www.srtalliance.org/) router: any number of
SRT inputs, any number of SRT outputs, and a router-style crosspoint (each
output picks exactly one source, switchable live) connecting them — the same
mental model as a broadcast video router, applied to SRT streams instead of
SDI/HDMI.

## What it does

By default, routing is a **pure relay**: the crosspoint moves opaque payload
chunks from an input SRT connection to an output SRT connection with no
decode/re-encode, so switching is effectively free (no transcode cost, no
added latency beyond SRT's own buffering). That's the right behavior for the
common case — plain stream switching — but it's not the *only* thing a
source can be. The engine's source abstraction is intentionally payload-only
(see [docs/architecture.md](docs/architecture.md)), so special-purpose
sources that actually generate a stream — a still image, a local media
player, a scaler tap on another source — can register into the same
crosspoint as a "source" without the engine caring that they're not relayed
SRT. **Not built yet** — Phase 1 is relay-only; see
[docs/roadmap.md](docs/roadmap.md).

Control is a local web UI (a crosspoint grid: click a cell to route that
output from that source) backed by a small REST API. No auth/TLS — this is
meant to run on a trusted operations network, the same trust model as a
hardware router's control port.

## Status

**Phase 1 (current): relay-only crosspoint + web UI.** Working:

- SRT input/output as either `listener` (this router waits for a
  connection) or `caller` (this router dials out), each reconnecting on its
  own if the connection drops.
- The crosspoint engine (`crates/core`) — output-follows-route-change
  behavior is unit tested.
- A local web UI (`crates/web`) — grid of outputs x sources, click to route,
  updated live over a websocket (`GET /ws`) with a REST poll (`GET
  /api/state`) as first paint / fallback.
- Routing changes optionally persist to disk (`[state]` in the config) and
  reload on restart, overriding each output's `default_source`.
- CI (GitHub Actions) runs `fmt --check`, `clippy -D warnings`, and the full
  test suite on every push/PR.
- Verified locally, not just compiled: `cargo test` passes, including
  integration tests (`crates/srt-io/tests/relay.rs`) that relay real SRT
  protocol traffic end-to-end through the crosspoint using `srt-tokio`
  clients as the encoder/decoder — one test also exercises a **live
  re-route mid-stream over an already-established SRT connection**.
  Separately confirmed by hand: running the binary against
  `config/example.toml` binds real UDP/SRT listener sockets (via `lsof`),
  the REST API and a real browser click both drive live crosspoint changes,
  the websocket push updates the grid with no client-side polling, and a
  persisted route survives a real process restart.

**Not yet done:** no test against a real third-party SRT
encoder/decoder or over a real (non-loopback) network path — only local
testing so far, still the main open gap. Also missing: special-purpose
sources (stills/media player/scaler), auth on the web UI/API, external
control API/Companion integration. See [docs/roadmap.md](docs/roadmap.md)
for the full phased plan.

## Quick start

```sh
cargo run --bin srtrouter -- --config config/example.toml
```

Then open `http://localhost:8080` for the crosspoint grid. Edit
[config/example.toml](config/example.toml) (or point `--config` at your own
file) to declare your actual inputs/outputs — see the comments in that file
for the config format.

## Architecture

See [docs/architecture.md](docs/architecture.md) for the source/output/
crosspoint model and how the relay-vs-generated source distinction is meant
to extend later without changing the core engine.

## Roadmap

See [docs/roadmap.md](docs/roadmap.md).
