# Roadmap

## Phase 1 — relay crosspoint + web UI — done

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
      `crates/router` has an optional `ndi` Cargo feature: build with
      `cargo run --features ndi` and (a) add `mode = "receiver"` /
      `"sender"` inputs/outputs to the TOML config (`config::Transport`,
      an untagged enum picking `srt_io::Endpoint` vs `ndi_io::Endpoint` by
      their disjoint `mode` values — existing SRT-only TOML files need no
      changes), or (b) add/remove them live via `POST`/`DELETE
      /api/manage/sources|outputs` — the same untagged-by-`mode` trick in
      `management::EndpointRequest`. `GET /api/manage/transports` reports
      which kinds this build supports (`["srt"]` or `["srt","ndi"]`); the
      web UI fetches it on load to decide whether to enable the NDI option
      in its transport dropdowns, rather than guessing. Verified live: NDI
      source added through the actual browser UI, confirmed registered
      with kind `"ndi"` via the API. Excluded from default CI either way
      (needs the real SDK installed, which CI can't do).
      Fixed a real bug found while testing this:
      `ndi-io`'s source-discovery loop didn't check its cancellation token
      at all, so a removed NDI input whose target never appeared would
      leak its blocking thread forever — now bounded to the discovery poll
      interval (~5s worst case).
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
- [x] **OMT** — `crates/omt-io`, hand-written FFI (`src/sys.rs`) against
      the real `libomt` C SDK (MIT-licensed, no bindgen — the API is small
      and stable enough that transcribing `libomt.h` directly was less
      risk than adding a bindgen step). Same `spawn_input`/`spawn_output`
      shape and `Bytes`-envelope pattern as `ndi-io`
      (`src/envelope.rs`). Requires `OMT_LIB_DIR` pointed at a
      `libomtnet` release's `Libraries/<platform>` folder (no standard
      install location exists for OMT the way NDI has one) — real
      workspace member, excluded from `default-members`/CI same as
      `ndi-io`. Verified for real: `crates/omt-io/tests/relay.rs` drives
      an actual OMT sender and receiver (raw `sys::` calls, not this
      crate's own code — an independent check on the envelope's fidelity)
      against `spawn_input`/`spawn_output`, consistently passing (~2.1s,
      3/3 runs). Every lesson from `ndi-io`'s test — continuous-framerate
      sender thread, `rt.shutdown_background()` — was applied from the
      start here rather than re-discovered.
      **Important constraint found while scoping the router wiring**:
      `config::Transport` and `management::EndpointRequest` disambiguated
      SRT from NDI via serde's `untagged` enum trying each variant until
      one's required fields match — this worked because SRT's `mode`
      values (`listener`/`caller`) are disjoint from NDI's
      (`receiver`/`sender`). OMT's `Endpoint` also uses `receiver`/`sender`,
      **and** its `Sender` variant is shape-identical to NDI's
      (`{mode: "sender", name: "..."}`) — untagged resolution can't tell
      them apart by content, it would silently always pick whichever
      variant is listed first in the enum, misrouting one transport's
      config as the other whenever both `ndi` and `omt` features are
      enabled together.
- [x] **OMT wired into the router** — resolved the constraint above by
      switching both `config::Transport` and `management::EndpointRequest`
      from `#[serde(untagged)]` to `#[serde(tag = "transport", rename_all =
      "lowercase")]`: every `[[inputs]]`/`[[outputs]]` entry and every
      `POST /api/manage/sources|outputs` body now names an explicit
      `transport = "srt" | "ndi" | "omt"`. Internally-tagged enums nest
      correctly inside `#[serde(flatten)]` fields and can themselves wrap
      another internally-tagged enum (SRT's own `mode`-tagged `Endpoint`),
      so this cost no expressiveness. `omt` is a new opt-in Cargo feature
      (`--features omt`, or `--features ndi,omt` together), wired into
      `main.rs`/`management.rs`/`available_transports()` the same way `ndi`
      is, and the web UI's transport dropdown now offers OMT (disabled
      until `GET /api/manage/transports` confirms the running binary
      supports it, same pattern as NDI). Added a regression test,
      `ndi_and_omt_sender_requests_are_not_confused_with_each_other`, that
      posts an NDI sender and an OMT sender with the same `name` and
      asserts each is dispatched to its own transport — passes, proving the
      tag actually resolves the collision rather than just moving it.
      Verified for real: default (SRT-only), `--features ndi`, `--features
      omt`, and `--features ndi,omt` all build and `cargo test` clean; a
      running `--features ndi,omt` binary correctly reports
      `["srt","ndi","omt"]` from `/api/manage/transports`, and adding an
      OMT source/destination through the actual browser UI produces the
      correct `omt`-badged grid rows with the route live.
      Separately hit and fixed a linking issue while verifying this without
      `DYLD_LIBRARY_PATH` set: `omt-io`'s own `build.rs` can only rpath its
      *own* test binaries (Cargo's unsuffixed `cargo:rustc-link-arg` never
      propagates to a dependent package's binary, and the suffixed
      `-bins` variant turned out to require the *emitting* package itself
      to have a `[[bin]]` target — it errors otherwise, which a lib-only
      crate like `omt-io` never has). Fixed by giving `crates/router` its
      own `build.rs` that emits the same rpath arg again, scoped to
      `srtrouter`'s actual `[[bin]]` target, whenever the `omt` feature and
      `OMT_LIB_DIR` are both set.

## Phase 2 — special-purpose sources — done

The core engine's `Source` abstraction (see [architecture.md](architecture.md))
supports these without any change to `crates/core` — each is a new producer
that calls `Crosspoint::register_source` and publishes chunks it generated
instead of relayed. All three are built in `crates/media-io`:

- [x] **Stills source** — loop a static image, encoded as a continuous
      MPEG-TS stream via a real `ffmpeg` child process
      (`-loop 1 -i image -f mpegts pipe:1`).
- [x] **Media player source** — play a local file (loop or one-shot) into
      the crosspoint as a source, same ffmpeg-child-process approach
      (`-stream_loop -1` when looping).
- [x] **Scaler source** — take another registered source, decode, rescale,
      re-encode, and publish the result as a new source (the one case where
      "pure relay" isn't the point — this is deliberately a real transcode
      path). Subscribes to the upstream source's own `Bytes` and feeds them
      into ffmpeg's `pipe:0`; retries (same fixed-delay pattern as SRT's
      reconnect loop) if the upstream source doesn't exist yet or
      disappears.

**The ffmpeg-vs-gstreamer-vs-native decision, resolved**: `ffmpeg` as a
child process, for three reasons found while scoping this in practice.
First, it needs no proprietary SDK or build-time linking at all (unlike
NDI/OMT) — it's a pure runtime dependency, checked by trying to spawn it —
so `media-io` carries no Cargo feature gate and stays a normal
`default-members`/CI-tested crate, unlike `ndi-io`/`omt-io`. Second, and
initially a surprise: this project's own dev-machine ffmpeg (Homebrew) has
no `srt` protocol support at all (`ffmpeg -protocols` lists `srtp`, not
`srt` — the `--enable-libsrt` build flag isn't Homebrew's default), so
asking ffmpeg to speak `srt://` directly would have made the whole feature
dependent on a non-default ffmpeg build. Piping raw MPEG-TS over
`pipe:1`/`pipe:0` sidesteps that entirely — works with any baseline ffmpeg
install, and is actually simpler than a network round-trip since the bytes
are already local to this process. Third, that same raw-pipe design means
these sources publish exactly the same wire format an SRT relay does (no
envelope, unlike NDI/OMT) — a stills slate can feed a live SRT output with
literally zero transcoding, which needed one deliberate fix: the web
crate's cross-kind route check (`crates/web/src/lib.rs`) used to require
exact kind-string equality, which would have wrongly blocked `media` ->
`srt` routing; it now groups `srt`/`media` into one payload-compatible
class while keeping NDI/OMT's real envelopes isolated (see
[architecture.md](architecture.md)'s media-io section for the details).

Also needed splitting `config::Transport`/`management::EndpointRequest`
(previously one enum shared by both inputs and outputs) into
`InputTransport`/`SourceEndpointRequest` (media included) and
`OutputTransport`/`EndpointRequest` (media excluded), since none of
stills/media-player/scaler make sense as an output — this makes routing a
`"transport":"media"` output request a compile-time impossibility on the
config side and a `422` on the REST side, rather than a runtime special
case to remember.

Verified for real: `crates/media-io/tests/relay.rs` generates its own test
assets via ffmpeg's `lavfi` synthetic sources (no checked-in binary
fixtures) and checks genuine MPEG-TS sync bytes (`0x47` every 188 bytes)
come out the other end of the crosspoint for all three source types,
including a scaler-recovers-once-its-upstream-appears-late test. CI
installs `ffmpeg` explicitly (`.github/workflows/ci.yml`) rather than
assuming the runner image bundles it. Live end-to-end: adding real stills/
media-player/scaler sources through the running web UI (backed by real
generated test images/video, not mocks) produces `media`-badged grid rows
that route cleanly into a live SRT output.

## Phase 3 — operational hardening (current)

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
