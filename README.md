# srt-router

> **AI-assisted project.** This codebase was created with [Claude](https://claude.com/claude-code)
> (Anthropic), directed and reviewed by a human author. The relay/crosspoint
> engine and web UI have been exercised locally — including integration
> tests that relay real SRT and NDI protocol traffic end-to-end and a live
> crosspoint switch via the web UI, see [Status](#status) — but **not yet
> run against real-world third-party SRT/NDI encoders/decoders or over an
> actual (non-loopback) network path**. Review before relying on it for
> anything live.

A crosspoint-based [SRT](https://www.srtalliance.org/) router: any number of
SRT inputs, any number of SRT outputs, and a router-style crosspoint (each
output picks exactly one source, switchable live) connecting them — the same
mental model as a broadcast video router, applied to SRT streams instead of
SDI/HDMI.

![srt-router architecture: SRT inputs feeding crosspoint-core's per-source broadcast channels, a routing table mapping each output to one source, out to SRT outputs](docs/diagrams/architecture.svg)

<img src="docs/screenshots/crosspoint-grid.png" alt="The crosspoint web UI: a grid of outputs (program, preview) by sources (cam1, cam2, remote-feed), each labeled with its transport kind and a remove control, plus Add source / Add destination buttons below" width="560">

*The web UI above is a real screenshot of the router running locally against
[config/example.toml](config/example.toml), not a mockup — captured while
verifying the routing/persistence/websocket/add-remove behavior described in
[Status](#status) below.*

## What it does

By default, routing is a **pure relay**: the crosspoint moves opaque payload
chunks from an input SRT connection to an output SRT connection with no
decode/re-encode, so switching is effectively free (no transcode cost, no
added latency beyond SRT's own buffering). That's the right behavior for the
common case — plain stream switching — but it's not the *only* thing a
source can be. The engine's source abstraction is intentionally payload-only
(see [docs/architecture.md](docs/architecture.md)), so special-purpose
sources that actually generate a stream — a still image, a local media
player, a scaler tap on another source — register into the same crosspoint
as a "source" without the engine caring that they're not relayed SRT.
**Built**: `crates/media-io` runs `ffmpeg` as a child process for all
three (stills/media-player/scaler), publishing plain MPEG-TS `Bytes` — the
same wire format an SRT relay carries — so a stills slate can feed a live
SRT output directly, no transcoding step required. No SDK, no Cargo
feature: just `ffmpeg` on `PATH` at runtime. See [Status](#status).

Control is a local web UI backed by a small REST API: a crosspoint grid
(click a cell to route that output from that source), plus **Add
source**/**Add destination** menus and a remove control on every row/column
for adding or tearing down SRT inputs/outputs at runtime — not just what was
in the TOML config at startup. No auth/TLS — this is meant to run on a
trusted operations network, the same trust model as a hardware router's
control port.

**Transports beyond SRT:** `crates/ndi-io` is a real, tested NDI transport
and `crates/omt-io` is the same idea for
[OMT](https://openmediatransport.org/) — a genuinely open, MIT-licensed
alternative to NDI — implemented via hand-written FFI against the real SDK
(requires `OMT_LIB_DIR`, no bindgen). Both are fully wired into the router:
usable from the TOML config **and** the runtime add-source/add-destination
REST API **and** the web UI's Add source/Add destination menus, behind their
own opt-in Cargo features (`cargo run --features ndi`, `--features omt`, or
both together). `crates/media-io` (stills/media-player/scaler, see
[What it does](#what-it-does)) needs no feature — just `ffmpeg` on `PATH`.
Every input/output entry — TOML or REST — now needs an explicit
`transport = "srt" | "ndi" | "omt" | "media"` tag; see
[config/example.toml](config/example.toml).

## Status

**Phase 1: relay-only crosspoint + web UI, dynamic add/remove — done.**
**Phase 2 (current): special-purpose (non-relay) sources — done.**
Working:

- SRT input/output as either `listener` (this router waits for a
  connection) or `caller` (this router dials out), each reconnecting on its
  own if the connection drops.
- The crosspoint engine (`crates/core`) — output-follows-route-change
  behavior is unit tested.
- A local web UI (`crates/web`) — grid of outputs x sources, click to route,
  updated live over a websocket (`GET /ws`) with a REST poll (`GET
  /api/state`) as first paint / fallback.
- **Runtime add/remove**: `POST`/`DELETE /api/manage/sources` and
  `/api/manage/outputs` (`crates/router/src/management.rs`) spawn or tear
  down an SRT input/output on the fly — the same code path the static TOML
  config uses at startup, so a config-declared source is exactly as
  removable as one added later. Backed by
  [`tokio_util::sync::CancellationToken`](https://docs.rs/tokio-util) per
  task (added to `srt-io`) so removal actually stops the task and frees the
  socket, not just forgets about it. The web UI exposes this as **Add
  source**/**Add destination** forms plus a remove control per row/column.
- Routing changes optionally persist to disk (`[state]` in the config) and
  reload on restart, overriding each output's `default_source`.
- `crates/ndi-io`: a real NDI transport using
  [grafton-ndi](https://github.com/GrantSparks/grafton-ndi) (Apache-2.0)
  against the actual NDI SDK, with its own integration test driving a real
  NDI sender and receiver against it, consistently passing. Fully wired into
  `srtrouter`'s TOML config, the runtime add/remove REST API, **and** the web
  UI's Add source/Add destination menus, behind an opt-in `ndi` Cargo feature
  (`cargo run --features ndi`).
- `crates/omt-io`: a real, tested [OMT](https://openmediatransport.org/)
  transport via hand-written FFI against the OMT SDK (`OMT_LIB_DIR`, no
  bindgen), with its own relay integration test. Wired in exactly the same
  way as NDI — TOML config, REST API, web UI menus — behind an opt-in `omt`
  Cargo feature (`cargo run --features omt`). Both features can be enabled
  together (`--features ndi,omt`); an explicit `transport` tag on every
  input/output disambiguates them even where their endpoint shapes are
  otherwise identical (NDI's and OMT's `Sender { name }`, in particular —
  see [docs/roadmap.md](docs/roadmap.md) for why that mattered).
- `crates/media-io`: stills, a local media player, and a decode/rescale/
  re-encode scaler tap, each running real `ffmpeg` as a child process and
  publishing plain MPEG-TS `Bytes` — no envelope, no proprietary SDK, no
  Cargo feature (just `ffmpeg` on `PATH`). Fully wired into the TOML
  config, the REST API, and the web UI's Add source menu (source-only —
  none of these make sense as an output). A `media` source routes straight
  into an SRT output with no transcoding step, since they share the same
  raw-MPEG-TS payload class; the web crate's cross-kind route check
  encodes that explicitly rather than requiring exact kind-string equality.
  Scaler additionally consumes another registered source's own `Bytes`
  (subscribe, pipe into ffmpeg's stdin) — the one non-relay path here that
  really does transcode.
- CI (GitHub Actions) runs `fmt --check`, `clippy -D warnings`, and the full
  test suite on every push/PR — SRT (+media) only (`ndi-io`/`omt-io` need
  real SDKs CI can't install, so they're real workspace members but
  excluded from `default-members`; `media-io` needs only `ffmpeg`, which CI
  installs explicitly — see [docs/architecture.md](docs/architecture.md)).
- Verified locally, not just compiled: `cargo test` passes — default
  (SRT+media), `--features ndi`, `--features omt`, and `--features
  ndi,omt` all build and pass clean — including integration tests
  (`crates/srt-io/tests/relay.rs`, `crates/ndi-io/tests/relay.rs`,
  `crates/omt-io/tests/relay.rs`, `crates/media-io/tests/relay.rs`) that
  relay real protocol traffic (or, for media-io, real ffmpeg-produced
  MPEG-TS, byte-checked for valid sync bytes) end-to-end through the
  crosspoint — one SRT test also exercises a **live re-route mid-stream
  over an already-established connection**, and one media-io test proves
  the scaler recovers once its upstream source appears late. Separately
  confirmed by hand: running the binary against `config/example.toml` binds
  real UDP/SRT listener sockets (via `lsof`), adding a source through the
  web UI binds a new one live and removing it frees the port (also via
  `lsof`), the REST API and a real browser click both drive live crosspoint
  changes, the websocket push updates the grid with no client-side polling,
  a persisted route survives a real process restart, adding an OMT
  source/destination through the running web UI produces the correct
  `omt`-badged rows on the grid, and adding real stills/media-player/scaler
  sources through the running web UI (backed by real generated test
  images/video) produces `media`-badged rows that route cleanly into a
  live SRT output.

**Not yet done:** no test against a real third-party SRT/NDI/OMT encoder or
decoder, or over a real (non-loopback) network path — only local testing so
far, still the main open gap. Also missing: auth on the web UI/API,
external control API/Companion integration. See
[docs/roadmap.md](docs/roadmap.md) for the full phased
plan.

## Quick start

```sh
cargo run --bin srtrouter -- --config config/example.toml
```

Then open `http://localhost:8080` for the crosspoint grid. Edit
[config/example.toml](config/example.toml) (or point `--config` at your own
file) to declare your actual inputs/outputs — see the comments in that file
for the config format.

## Desktop app

Prefer not to touch the terminal? A small menu-bar app lets you pick the network
interface + port, Start/Stop the server, and open the web UI. The `srtrouter`
server is bundled inside, so it's a single download — nothing to install or wire
up. Grab the `.dmg` from
[Releases](https://github.com/allansargeant/srt-router/releases), or see
[launcher/](launcher/) to build it.

<p align="center"><img src="launcher/docs/panel.png" width="300" alt="SRT Router desktop app"></p>

## Architecture

See [docs/architecture.md](docs/architecture.md) for the source/output/
crosspoint model and how the relay-vs-generated source distinction is meant
to extend later without changing the core engine.

## Unsigned builds — macOS Gatekeeper & Windows SmartScreen

The release binaries are **not code-signed or notarized** — that needs paid
Apple / Windows developer certificates this project doesn't carry. The binaries
are safe to run; the OS just can't verify a publisher, so it warns you the first
time. Here's how to get past that, and how to sign them yourself if you'd rather.

### macOS

These are command-line binaries, so clear the quarantine flag in Terminal. After
extracting the archive, `cd` into it and run:

```sh
xattr -dr com.apple.quarantine ./<binary>   # remove the "unverified developer" flag
chmod +x ./<binary>                          # ensure it's executable
./<binary> --help
```

Or run it once, let macOS block it, then go to **System Settings → Privacy &
Security** and click **Open Anyway**.

### Windows

Running the `.exe` may show **"Windows protected your PC"** (SmartScreen) — click
**More info → Run anyway**. If you extracted it from a `.zip`, you can clear the
flag first: right-click the `.exe` → **Properties** → tick **Unblock** → **OK**,
or in PowerShell `Unblock-File .\<binary>.exe`.

### Linux

No signing gate — just `chmod +x ./<binary>` (or install the `.deb`/`.rpm`).

### Signing it yourself (optional)

On macOS an *ad-hoc* signature stops repeated prompts on your own machine (it is
**not** notarization — it won't clear Gatekeeper on someone else's Mac):

```sh
codesign --force --sign - ./<binary>
```

Clearing the warnings for redistribution needs paid certificates: an **Apple
Developer Program** membership ($99/yr) + a *Developer ID Application* cert with
`xcrun notarytool` on macOS, or an **Authenticode** code-signing certificate from
a CA (`signtool sign`) on Windows.

## Roadmap / TODO

Full phased plan in [docs/roadmap.md](docs/roadmap.md). Main open items:

- [ ] **Real-world testing** — against a third-party SRT/NDI/OMT encoder/decoder and over a real (non-loopback) network path; the main open gap.
- [x] **NDI and OMT live in the web UI, config, and REST API** — both fully wired behind their own opt-in Cargo features (`ndi`, `omt`), disambiguated by an explicit `transport` tag per input/output.
- [x] **Special-purpose sources** — stills, local media player, scaler tap, all built on real `ffmpeg` child processes and live in the web UI's Add source menu, config, and REST API.
- [ ] **Auth/TLS** on the web UI/API.
- [ ] **External control API / Bitfocus Companion** integration.
