# srt-router API reference

`srtrouter` serves a small JSON HTTP API and a WebSocket, on the address given by
`[web] bind` in the config file (the example uses `0.0.0.0:8080`).

**There is no authentication.** Anyone who can reach the port can re-route any output, and
add or remove sources and outputs. Bind to a management interface or firewall it.

---

## Crosspoint API

### `GET /`
The web UI (a single embedded HTML page).

### `GET /api/state`
The whole crosspoint state.

```json
{
  "sources": ["cam1", "cam2", "remote-feed"],
  "outputs": ["program", "preview"],
  "routes": { "program": "cam1", "preview": "cam2" }
}
```

`routes` maps **each output to exactly one source** — that's the crosspoint model. An output
has one source; a source may feed many outputs.

### `POST /api/route`
Route an output to a source. This is the live switch.

```json
{ "output": "program", "source": "cam2" }
```

Response:
```json
{ "ok": true }
```

On rejection, `ok` is `false` and `error` explains why. **Note this returns HTTP 200 with
`ok: false`** rather than a 4xx — check the body, not just the status.

The one rejection you'll meet in practice is a payload mismatch:

```json
{ "ok": false,
  "error": "can't route a <src> source to a <out> output without transcoding" }
```

The router refuses to connect incompatible payload kinds rather than emitting a stream the
far end can't decode. Media sources publish plain MPEG-TS — the same wire format SRT relays
— so a stills slate can feed an SRT output with no transcoding.

### `GET /ws`
WebSocket. Pushes the same `GET /api/state` object **on connect and again whenever the
routing changes**, so a UI updates live without polling.

---

## Management API

Add and remove sources and outputs at runtime.

**Config-defined and API-added entities are the same thing once running.** Both go through
the same `spawn_input`/`spawn_output` + registry insert. A source declared in the config file
is exactly as removable via the API as one added later — there is no "permanent" tier.

### `GET /api/manage/transports`
Which transports this binary actually supports:

```json
["srt", "media"]
```

This is **build-dependent**. `ndi` and `omt` appear only if the binary was compiled with the
matching Cargo feature; `media` is always present (it needs only `ffmpeg` on `PATH` at
runtime). Query this rather than assuming.

### `GET /api/manage/sources` · `GET /api/manage/outputs`
```json
[ { "id": "cam1", "kind": "srt" } ]
```

### `POST /api/manage/sources` · `POST /api/manage/outputs`
Create one at runtime. The body mirrors the config file's shape for that entity — a
`transport` tag plus that transport's own fields — and dispatch is by the `transport` tag.

`POST /api/manage/outputs` **requires `default_source`**.

**`media` is rejected for outputs** — media is an input-only transport; it has no output
side.

A duplicate `id` is a **conflict**.

### `DELETE /api/manage/sources/:id` · `DELETE /api/manage/outputs/:id`
Removes the entity and **frees its port** — removal genuinely tears down the listener, it
isn't just a registry delete. An unknown `id` is **not found**.

---

## Configuration file

Passed with `-c` / `--config`, defaulting to `config/example.toml`. TOML.

```toml
[web]
bind = "0.0.0.0:8080"

[state]                      # OPTIONAL
path = "state/routes.json"

[[inputs]]
id = "cam1"
transport = "srt"
mode = "listener"
bind = "0.0.0.0:5001"

[[outputs]]
id = "program"
transport = "srt"
mode = "listener"
bind = "0.0.0.0:6001"
default_source = "cam1"
```

### `[state]` changes restart behaviour — know which you want

- **Present** — routing changes made via the UI/API are persisted and **reloaded on startup,
  overriding every output's `default_source`**.
- **Absent** — routing is in-memory only and **resets to `default_source` on every restart**.

### Transports and modes

| `transport` | `mode` values | Requires |
|---|---|---|
| `srt` | `listener`, `caller` | — |
| `ndi` | `receiver`, `sender` | built with `--features ndi` + NDI SDK |
| `omt` | `receiver`, `sender` | built with `--features omt` + libomt |
| `media` | `stills`, `mediaplayer`, `scaler` | `ffmpeg` on `PATH` (inputs only) |

`listener` means *this router waits for the far end to connect*; `caller` means *this router
dials out*. A common setup is encoders calling **in** to fixed input ports and decoders
listening for the router to call **out** — but any combination works.

**Transports mix freely.** An NDI, OMT or media source can feed an SRT output and vice
versa, exactly as easily as staying within one transport.

Media inputs default to 1280x720 if `width`/`height` are omitted, and `loop_playback`
defaults to `true`. The `scaler` mode rescales/re-encodes an existing source by `id`.

---

## Logging

Standard `tracing` env filter, default `info`:

```bash
RUST_LOG=debug cargo run -p router
```
