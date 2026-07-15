# Architecture

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
 [future] stills source      ──▶ broadcast::Sender<Bytes> ──┤   routing table
 [future] media player       ──▶ broadcast::Sender<Bytes> ──┘  (output id -> source id)
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
2. **Future non-relay sources** — a stills source, a local media player, or
   a scaler tap on another source all have the *same* registration API
   (`Crosspoint::register_source`), they just publish chunks they generated
   instead of chunks they received from a socket. From the crosspoint's (and
   every output's) point of view, there is no difference between "relayed
   SRT source" and "locally generated source" — both are just something
   calling `.send(bytes)` on a broadcast channel. Building one of these is a
   new module in a new crate (or in `crates/srt-io` if it still needs SRT
   framing), not a change to `crates/core`.

The tradeoff this buys: Phase 1 switching between two *relayed* sources is
not pixel-seamless — the output's downstream decoder will see a
mid-GOP cut and re-lock at the next keyframe, same as any real SRT/IP
router (Zixi, Haivision, Dektec-class boxes) that switches at the transport
level rather than the pixel level. A source that decodes/re-encodes (the
future scaler/media-player case) *can* be made seamless at its own output,
at the cost of actually running a media pipeline for that one path — see
[roadmap.md](roadmap.md).

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
  server.

## Config format

Each input/output is one SRT endpoint in either mode:

```toml
[[inputs]]
id = "cam1"
mode = "listener"       # this router waits for the connection
bind = "0.0.0.0:5001"

[[inputs]]
id = "remote-feed"
mode = "caller"         # this router dials out
connect = "203.0.113.10:5000"
```

Outputs are the same shape plus `default_source` (what they're routed from
at startup if nothing better is known — see below).

Optionally, a top-level `[state]` section with a `path` enables persisting
routing changes to disk: every time a route changes, the full output ->
source table is written to that JSON file (write-then-rename, so a crash
mid-write can't leave a corrupt file behind); on startup, any persisted
route for a configured output overrides that output's `default_source`.
Omit `[state]` to keep routing in-memory only, as before — every restart
then resets to each output's `default_source`. See
`crates/router/src/state.rs`.
