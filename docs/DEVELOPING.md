# Developing srt-router

Setup, build, test and extension guide. For the architectural *why*, read
[`AGENTS.md`](../AGENTS.md); for the wire/HTTP surface, [`API.md`](API.md).

---

## The first thing that will happen to you

**A plain `cargo build` on the workspace fails.** You'll get:

```
error: failed to run custom build command for `omt-io`
  OMT_LIB_DIR is not set. Point it at the directory containing libomt's dynamic
  library, extracted from a libomtnet release zip's Libraries/<platform> folder
```

**This is not a broken checkout.** `omt-io` links an external library that isn't vendored
into the repo. You have two options:

```bash
# 1. Work on the subset that needs no external SDKs (usually what you want)
cargo build -p crosspoint-core -p router -p srt-io -p web
cargo test  -p crosspoint-core -p router -p srt-io -p web

# 2. Or supply the library
export OMT_LIB_DIR=/path/to/libomtnet/Libraries/macos
cargo build
```

Releases: <https://github.com/openmediatransport/libomtnet/releases>

`ndi-io` has the same shape of dependency — it needs the NDI SDK installed to compile.

**Neither is required for the core router.** SRT, the crosspoint and the web UI all build
with no external SDK.

---

## Commands

```bash
cargo build
cargo test
cargo run -p router                       # uses config/example.toml
cargo run -p router -- -c my-config.toml
cargo clippy --all-targets --all-features
```

`--all-features` here means NDI **and** OMT must be available. On a machine without them,
lint per-crate instead. (Note this differs from the sibling `openstage` repo, where
`--all-features` is explicitly forbidden for the same reason — here it's the documented
command, but it still needs the SDKs.)

Logging is `tracing` with an env filter, default `info`:
```bash
RUST_LOG=debug cargo run -p router
```

---

## Layout

```
crates/
  core/      Crosspoint/matrix model and shared types. The I/O trait lives here.
  router/    Main binary
    main.rs        Config load, spawn inputs/outputs, wire the registry
    config.rs      TOML config types
    management.rs  The /api/manage runtime add/remove API
  srt-io/    SRT backend
  ndi-io/    NDI backend      (feature `ndi`,  needs NDI SDK)
  omt-io/    OMT backend      (feature `omt`,  needs OMT_LIB_DIR)
  media-io/  Stills / file playback / scaler (needs ffmpeg at runtime)
  web/       Web UI + crosspoint API. static/index.html is include_str!'d in.
```

---

## The architectural rule

**Every I/O backend stays behind `core`'s I/O trait.**

The crosspoint's whole value is that the matrix doesn't know or care what a source *is* — a
camera over SRT, an NDI stream and a still image are interchangeable to it. A backend that
reaches into `router` directly, or that the crosspoint special-cases, breaks that.

Backends are gated by Cargo features and dispatched by a `transport` tag, so the binary
works with any subset compiled in. `available_transports()` reports what's actually present
— **never assume a transport exists**, in code or in the UI.

---

## Adding a transport

1. New crate `crates/<name>-io`, implementing `core`'s I/O trait, with `spawn_input` and (if
   it has an output side) `spawn_output`.
2. Add a Cargo feature if it needs an external SDK. If it needs only a runtime binary — as
   `media-io` needs ffmpeg — no feature is needed; keep it always-compiled.
3. Add variants to `InputTransport` / `OutputTransport` in `router/src/config.rs`, behind
   the feature.
4. Dispatch in `main.rs` and in `management.rs`'s `add_source`/`add_output`.
5. Add it to `available_transports()`.
6. Document it in [`API.md`](API.md) and `config/example.toml`.

**Input-only transports are legitimate** — `media` is one, and outputs reject it explicitly.
There's a test for that; add the equivalent for yours.

---

## Testing

`management.rs` carries the pattern worth following. Its tests exercise real behaviour
rather than mocking it out:

- `add_then_remove_source_really_binds_and_frees_the_port` — asserts the port is genuinely
  bound and genuinely released, not merely that a registry entry appeared and vanished.
- `add_duplicate_id_is_a_conflict`, `remove_unknown_source_is_not_found` — the error paths.
- `add_output_requires_default_source_field`.
- `transports_lists_srt_and_ndi_iff_the_feature_is_on` — feature gating is itself tested.
- `ndi_and_omt_sender_requests_are_not_confused_with_each_other` — two structurally similar
  transports must not cross-dispatch.
- `media_transport_is_rejected_for_outputs`.

Integration tests relay **real SRT and NDI protocol traffic end-to-end**. That bar is the
reason to trust the relay path at all — keep it.

**What testing has not covered, and must not be claimed:** no real third-party encoder or
decoder, and no real network path. Everything so far is loopback and synthetic.

---

## Things that will surprise you

- **`POST /api/route` returns HTTP 200 with `{"ok": false}`** on a rejected route, not a
  4xx. Check the body.
- **Payload compatibility is enforced before routing.** `payload_compatible` refuses to
  connect kinds that would need transcoding.
- **Config-declared and API-added entities are indistinguishable at runtime.** Both go
  through the same spawn + registry insert, so a config-file source is just as removable via
  the API. This is deliberate — don't add a "protected" tier without a good reason.
- **`[state]`, when present, overrides every output's `default_source` at startup.** A
  developer wondering why their config change had no effect has usually got a stale
  `state/routes.json`.
- **The web UI is a single `include_str!`'d HTML file** (`crates/web/static/index.html`) —
  no bundler, no npm. Edit it directly.
- **The `/ws` handler polls the crosspoint for changes** on an interval and pushes on
  change, rather than being event-driven.

---

## Releasing

Multi-platform release CI. **Cross-compile macOS x86_64 on `macos-14`, never `macos-13`** —
the Intel runners are retired.

Ships as its own desktop app via **av-launcher** (Tauri tray shell, server embedded). Note
the macOS Gatekeeper trap: for an unsigned `.app` bundling helper binaries, approving the app
does **not** unquarantine its payload — helpers are SIGKILLed silently, presenting as "the
app opens but the server never starts".

Public repo, ships a user-facing AI-assisted disclaimer. "Commit" means commit **and** push.
