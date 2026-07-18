# Architecture

![srt-router architecture: SRT inputs feeding crosspoint-core's per-source broadcast channels, a routing table mapping each output to one source, out to SRT outputs](diagrams/architecture.svg)

## The core model

Three concepts, deliberately kept separate:

- **Source** — anything that can publish a stream of opaque payload chunks
  (`Bytes`). Registering one gets you a `broadcast::Sender<Bytes>` to publish
  to; the crosspoint doesn't know or care what's inside the chunks or where
  they came from.
- **Output** — a sink for exactly one source at a time, selected by id.
  Registering one gets you a `watch::Receiver<SourceId>` that changes value
  whenever the crosspoint re-routes that output.
- **Crosspoint** (`crates/core`) — the registry and routing table connecting
  the two. It has no transport code in it at all: no networking, no SRT, no
  encode/decode. That's deliberate — it's the one piece every kind of source
  and output shares, so it has to stay generic.

```
 SRT input (listener/caller) ──▶ broadcast::Sender<Bytes> ──┐
 SRT input (listener/caller) ──▶ broadcast::Sender<Bytes> ──┤   crosspoint
 stills source (ffmpeg)      ──▶ broadcast::Sender<Bytes> ──┤   routing table
 media player (ffmpeg)       ──▶ broadcast::Sender<Bytes> ──┘  (output id -> source id)
                                                                    │
                                        watch::Receiver<SourceId> ◀─┘
                                                 │
                                                 ▼
                                   SRT output (listener/caller)
```

Each output task owns one SRT socket and runs a small loop
(`crates/srt-io/src/lib.rs::relay_out`): borrow the currently-routed source
id from its `watch::Receiver`, subscribe to that source's `broadcast`
channel, and forward every chunk it receives to the socket — until either
the watch fires (routing changed: break out, re-subscribe to the new
source) or the broadcast channel closes (source gone). This is why
switching is instantaneous and cheap: it's a channel resubscription, not a
new connection or a decode pipeline restart.

## Why payload-chunk, not "SRT packet" or "video frame"

The `Source`/`Output` boundary is typed as raw `Bytes`, not as anything
SRT-specific or video-specific. That's what lets:

1. **Phase 1 (relay)** — an SRT input's received chunk is published
   unmodified; an SRT output's received chunk is sent unmodified. No
   decode/re-encode, no dependency on knowing the payload is MPEG-TS (SRT
   itself is payload-agnostic — it just moves packets — so this router never
   needs to understand the video codec/container to relay it).
2. **Non-relay sources** (`crates/media-io`) — a stills source, a local
   media player, and a scaler tap on another source all have the *same*
   registration API (`Crosspoint::register_source`), they just publish
   chunks they generated (by running `ffmpeg` as a child process) instead of
   chunks they received from a socket. From the crosspoint's (and every
   output's) point of view, there is no difference between "relayed SRT
   source" and "locally generated source" — both are just something calling
   `.send(bytes)` on a broadcast channel.

The tradeoff this buys: switching between two *relayed* sources is not
pixel-seamless — the output's downstream decoder will see a mid-GOP cut and
re-lock at the next keyframe, same as any real SRT/IP router (Zixi,
Haivision, Dektec-class boxes) that switches at the transport level rather
than the pixel level. A source that decodes/re-encodes (media-io's scaler,
in particular) *can* be made seamless at its own output, at the cost of
actually running a media pipeline for that one path — see
[roadmap.md](roadmap.md).

## This isn't hypothetical — `crates/ndi-io` proves it

NDI is exactly the "future non-relay source" case above, except it arrived
before stills/media-player/scaler did. NDI has no single opaque payload the
way SRT/MPEG-TS does — a receiver hands you distinct video, audio, and
metadata frames, each with its own fields (resolution, pixel format, sample
rate, timecode...). `crates/ndi-io/src/envelope.rs` is the "turn a native
frame into a `Bytes` blob and back" logic the note above described in the
abstract: a small self-describing wire format (a kind byte, then that
frame's fields, then its raw data) that a receiver encodes into and a
sender decodes back out of. `crates/core` never links against `grafton-ndi`
or knows NDI exists — it's still just moving `Bytes`.

![One crosspoint, two ways of filling a Bytes chunk: SRT relays its socket payload straight through, NDI encodes a VideoFrame/AudioFrame/MetadataFrame into a self-describing envelope and decodes it back on the other side, both ending up in the same crosspoint-core Bytes broadcast channel](diagrams/multi-transport-envelope.svg)

The real consequence of this design, not just a footnote: an SRT source
can't be routed to an NDI/OMT output (or vice versa) without actual
transcoding — they're different envelopes, not just different wire formats
carrying the same thing. The router enforces this: cross-kind routes are
rejected at the API layer (`src/registry.rs` tracks each source/output's
transport kind; `POST /api/route` checks it matches before applying). Both
`crates/ndi-io` and `crates/omt-io` are wired into the `srtrouter`
binary/config/UI (see [roadmap.md](roadmap.md)) and independently tested —
`crates/ndi-io/tests/relay.rs` and `crates/omt-io/tests/relay.rs` each drive
an actual sender/receiver of their respective protocol against it, proving
the envelope pattern holds for both.

## media-io: the non-envelope case

Stills, media player, and scaler (`crates/media-io`) are the "future
non-relay sources" from the note above, made concrete — but unlike NDI/OMT,
they deliberately *don't* need an envelope. Each spawns `ffmpeg` as a child
process (`-f mpegts pipe:1`) and republishes its stdout, chunked into
188-byte-multiple slices (the conventional SRT/MPEG-TS payload alignment),
straight onto the crosspoint's broadcast channel — no frame decode/re-encode
of the *transport* layer, just plain MPEG-TS bytes, identical in shape to
what an SRT relay's socket hands over. ffmpeg itself is what's doing real
work (encoding a looped image, playing a file, or — for the scaler —
decoding/rescaling/re-encoding another source's own `Bytes`, fed into its
`pipe:0`), but the crosspoint-facing wire format needs no self-describing
envelope the way NDI/OMT's distinct video/audio/metadata frames do.

That has one concrete, easy-to-get-wrong consequence for cross-kind route
validation: a `media` source is payload-compatible with an `srt` output
(and vice versa) despite the different kind *strings*, because both carry
the same raw MPEG-TS — routing a stills slate straight into a live SRT
output needs no transcoding, which is the actual point of building it as a
raw pipe rather than an NDI/OMT-style envelope. `crosspoint-web`'s route
guard (`payload_compatible` in `crates/web/src/lib.rs`) encodes this
explicitly: it groups `srt`/`media` into one payload class and leaves
`ndi`/`omt` isolated from everything else (each other included), rather
than requiring exact kind-string equality the way the NDI/OMT case does.

Why ffmpeg as a subprocess rather than a Rust-native encode/decode crate:
mature, well-tested codec support with no proprietary SDK or FFI to
maintain (unlike NDI/OMT, `ffmpeg` needs no build-time linking at all — it's
a runtime dependency, checked by actually trying to spawn it, so
`media-io` carries no Cargo feature gate and is a normal `default-members`
crate). And specifically *not* asking ffmpeg to speak SRT itself
(`-f mpegts pipe:1`/`pipe:0` instead of an `srt://` URL): plenty of ffmpeg
builds — this project's own dev machine's Homebrew build included — are
compiled without libsrt support, so piping raw bytes works with any
baseline install and skips a network hop entirely, since the bytes are
already local to this process.

## Crate layout

- `crates/core` (`crosspoint-core`) — the engine described above. No I/O,
  no async runtime dependency beyond `tokio::sync` primitives. Unit tested
  in isolation.
- `crates/srt-io` (`srt-io`) — wraps [`srt-tokio`](https://github.com/russelltg/srt-rs)
  (a pure-Rust SRT implementation, no libsrt/C dependency) into
  `spawn_input`/`spawn_output`, each owning one socket and one reconnect
  loop (`listener`: re-`listen_on`; `caller`: re-`call`).
- `crates/web` (`crosspoint-web`) — an `axum` server: `GET /api/state`,
  `POST /api/route`, `GET /ws` (websocket push — sends the current state on
  connect and again whenever it changes), and a single embedded HTML/JS
  page (`crates/web/static/index.html`) that does one REST fetch for first
  paint, then switches to the websocket for live updates (falling back to a
  2s reconnect loop if the connection drops).
- `crates/router` (bin `srtrouter`) — loads a TOML config
  ([config/example.toml](../config/example.toml)), registers every
  configured input/output with a shared `Crosspoint`, and starts the web
  server. `src/registry.rs` tracks the transport kind + a
  `tokio_util::sync::CancellationToken` for every running source/output —
  both config-loaded and API-added ones go through the same
  `insert_source`/`insert_output` calls, so either is equally
  listable/removable. `src/management.rs` is the runtime add/remove REST
  API (`POST`/`DELETE /api/manage/sources`, `/api/manage/outputs`) that the
  web UI's Add source/destination forms and remove buttons call — merged
  into the same `axum::Router` as `crosspoint-web`'s routes via `.merge()`.
- `crates/ndi-io` (`ndi-io`) — the NDI transport: `spawn_input`/
  `spawn_output` with the same shape as `srt-io`'s, using
  [grafton-ndi](https://github.com/GrantSparks/grafton-ndi) against the
  real NDI SDK, with `src/envelope.rs` doing the frame<->`Bytes` conversion
  described above. Requires the actual NDI SDK installed to build (real
  bindgen against its headers) — it's a genuine workspace member
  (buildable/testable via `-p ndi-io`) but excluded from
  `default-members`, so it doesn't affect the default `cargo build`/`cargo
  test`/CI, which stay SRT-only. Wired into `crates/router` behind the `ndi`
  Cargo feature — config, runtime REST API, and web UI all reach it.
- `crates/omt-io` (`omt-io`) — the OMT transport (open, MIT-licensed
  protocol): same `spawn_input`/`spawn_output` shape again, hand-written FFI
  (`src/sys.rs`) transcribed directly from `libomt.h` (no bindgen — the C
  API is small and stable enough that this was less effort and more
  auditable than a build-time bindgen step), `src/envelope.rs` doing the
  same frame<->`Bytes` conversion. Requires `OMT_LIB_DIR` pointed at a
  `libomtnet` release's `Libraries/<platform>` folder to build (no standard
  install location exists for OMT the way NDI has one). Also excluded from
  `default-members`. Wired into `crates/router` behind the `omt` Cargo
  feature, the same way as NDI — including being usable simultaneously with
  it (`--features ndi,omt`). Because `omt-io` has no `[[bin]]` target of its
  own, its rpath (`-Wl,-rpath,$OMT_LIB_DIR`) only reaches its *own* test
  binaries via its `build.rs`; `crates/router/build.rs` emits the same
  rpath arg again for `srtrouter`'s actual `[[bin]]` target when the `omt`
  feature is on, which is what lets a built `srtrouter` binary find
  `libomt` at process startup without `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH`
  set.
- `crates/media-io` (`media-io`) — stills/media-player/scaler, see the
  dedicated section above. `spawn_input` runs `ffmpeg`, republishing its
  `pipe:1` MPEG-TS in packet-aligned chunks; the scaler additionally
  subscribes to another registered source and feeds its `Bytes` into
  ffmpeg's `pipe:0`. No SDK, no Cargo feature — a normal `default-members`
  crate, always wired into `crates/router`'s config/REST API/web UI (see
  `config::InputTransport::Media`, `management::SourceEndpointRequest`).
  Source-only: `config::OutputTransport` and `management::EndpointRequest`
  (used for outputs) have no `Media` variant, since none of stills/media-
  player/scaler make sense as something the router sends routed video *to*.

## Config format

Every input/output names an explicit
`transport = "srt" | "ndi" | "omt" | "media"` (NDI/OMT entries only take
effect when the router is built with the matching Cargo feature; `media`
needs no feature, just `ffmpeg` on `PATH`), plus that transport's own
`mode`:

```toml
[[inputs]]
id = "cam1"
transport = "srt"
mode = "listener"       # this router waits for the connection
bind = "0.0.0.0:5001"

[[inputs]]
id = "remote-feed"
transport = "srt"
mode = "caller"         # this router dials out
connect = "203.0.113.10:5000"

[[inputs]]
id = "ndi-cam"
transport = "ndi"
mode = "receiver"       # discover a source whose name contains this text
source_name = "SOME-MACHINE (Camera 1)"

[[inputs]]
id = "omt-cam"
transport = "omt"
mode = "receiver"       # discover a source whose discovery address contains this text
address = "SOME-MACHINE (Camera 1)"

[[inputs]]
id = "slate"
transport = "media"
mode = "stills"          # loop a static image; other modes are "mediaplayer"
image_path = "/opt/srtrouter/slate.png"   # (file_path, loop_playback) and
                                           # "scaler" (source: an existing source id)
```

Outputs are the same shape plus `default_source` (what they're routed from
at startup if nothing better is known — see below); NDI/OMT outputs use
`mode = "sender"` with a `name` field instead of `bind`/`connect`. `media`
has no output form at all — see `config::OutputTransport` in the crate
layout above.

The `transport` tag is load-bearing, not decorative: `InputTransport` in
`crates/router/src/config.rs` is a `#[serde(tag = "transport")]` enum rather
than an untagged one specifically because NDI's and OMT's `Sender { name }`
shapes are byte-for-byte identical (`{"mode": "sender", "name": "..."}`) —
untagged resolution can't tell those two apart by content alone, it would
silently always pick whichever variant is declared first in the enum. The
same tagging applies to the runtime REST API's `SourceEndpointRequest`/
`EndpointRequest` in `crates/router/src/management.rs`.

Optionally, a top-level `[state]` section with a `path` enables persisting
routing changes to disk: every time a route changes, the full output ->
source table is written to that JSON file (write-then-rename, so a crash
mid-write can't leave a corrupt file behind); on startup, any persisted
route for a configured output overrides that output's `default_source`.
Omit `[state]` to keep routing in-memory only, as before — every restart
then resets to each output's `default_source`. See
`crates/router/src/state.rs`.

## Runtime add/remove API

Config-loaded sources/outputs are the *starting* set, not the only
possible one. `crates/router/src/management.rs` exposes:

```
GET    /api/manage/sources              -> [{ "id": "cam1", "kind": "srt" }, ...]
POST   /api/manage/sources               { "id": "cam3", "transport": "srt", "mode": "listener", "bind": "0.0.0.0:5003" }
DELETE /api/manage/sources/:id
GET    /api/manage/outputs              -> [{ "id": "program", "kind": "srt" }, ...]
POST   /api/manage/outputs               { "id": "aux", "transport": "srt", "mode": "caller", "connect": "203.0.113.10:6000", "default_source": "cam1" }
DELETE /api/manage/outputs/:id
GET    /api/manage/transports           -> ["srt", "media", "ndi", "omt"]   (whichever Cargo features are on; media is always present)
```

`POST` is rejected with `409 Conflict` if the id already exists (checked
against the live `Crosspoint`, not just the registry, so it can't collide
with a config-loaded id either). `DELETE` on an unknown id is `404`. A
successful `DELETE` calls the entry's `CancellationToken` (which stops its
`spawn_input`/`spawn_output` task — for a `listener`, this actually frees
the bound socket, confirmed with `lsof` during testing, not assumed) and
then `Crosspoint::deregister_source`/`deregister_output`.

`POST /api/manage/sources` bodies are dispatched by their `transport` tag
(`SourceEndpointRequest` in `management.rs`, `#[serde(tag = "transport")]`)
to `srt_io::`, `ndi_io::`, `omt_io::`, or `media_io::spawn_input` —
whichever of `ndi`/`omt` are compiled in via Cargo feature (`media` is
always available). `POST /api/manage/outputs` uses the narrower
`EndpointRequest` (no `Media` variant — see `config::OutputTransport`), so
a `"transport":"media"` output body fails to deserialize (`422`) rather
than silently doing nothing. `GET /api/manage/transports` reports which
kinds the running binary actually supports; the web UI calls it on load and
only un-disables the matching `<option>`s in its transport dropdowns
(`media` needs no un-disabling — it's always on, and only ever offered in
the *source* dropdown, never the destination one, since the HTML itself has
no `media` `<option>` there).
