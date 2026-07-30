# srt-router's hosted demo

The router relays live SRT (and NDI/OMT) streams between endpoints on a network,
so there is nothing for a page on the public internet to route. What is hosted at
<https://srt-router.stoatworks-labs.com> is a **click-through demo**: the real,
unmodified crosspoint UI, replaying responses recorded from srt-router itself
running with `config/demo.toml`.

**The crosspoint really switches.** The UI redraws only from what the router
pushes down its WebSocket, so `record-demo.sh` performs all twelve
output×source switches against the running router and records its actual
reaction to each. Clicking a cell in the demo replays the router's own answer
for that exact switch.

Nothing is live: no stream is being relayed, and nothing is saved.

## Why it's recorded and not simulated

A hand-written fixture is a guess about what the software does, and guesses
drift away from the code. Everything the demo replays is a byte the router
actually produced.

One subtlety worth knowing before changing any of this: the recorder captures a
snapshot **immediately before** each write as well as after, and the shim
replays the *difference*. It has to. The recording session performs all twelve
switches in sequence, so a late snapshot also contains every earlier switch —
replaying one verbatim moved crosspoints the visitor never clicked. Diffing
isolates what each switch alone did, which is what makes clicking three cells in
a row behave the way the real router behaves.

## What's here

| File | What it is |
|---|---|
| `record-demo.sh` | Rebuilds everything: starts the router, records reads and all switches, assembles |
| `record-fixtures.mjs` | Records a running backend's responses and writes (vendored) |
| `demo-shim.js` | Replays the recording in the page over `fetch`/`WebSocket` (vendored) |
| `build-demo.sh` | Assembles `crates/web/static` + shim + fixtures into a site (vendored) |
| `serve-demo.py` | Serves it with a static host's headers, for local checking (vendored) |
| `demo-fixtures.json` | The recording. Regenerate it; don't hand-edit it |
| `dist/` | **Committed build output** — what Cloudflare Pages serves |

The vendored files come from `stoatworks-backend/pages-demo`. Fix them there and
copy out, or the copies drift.

## Rebuilding and publishing

```bash
demo/record-demo.sh                                       # record + assemble
demo/serve-demo.py --dir demo/dist    # check it locally first
git add demo/dist && git commit && git push   # Cloudflare publishes it
```

`config/demo.toml` deliberately uses **SRT only**. NDI and OMT need their
proprietary SDKs and their own Cargo features, so recording with them would show
a default build doing something it can't.

Cloudflare Pages publishes `demo/dist` from the repo with **no build command**.
It has to be committed: assembling the demo means running the app against its
mock devices and capturing what it says, which a build container can't do.

## Rules the demo has to keep

- **It always says it's a demo.** The banner isn't optional.
- **Only recorded behaviour.** If a control does nothing useful, record it —
  never hand-write plausible JSON, because that's how a demo starts showing
  behaviour the software doesn't have.
- **Adding/removing sources isn't recorded**, so those forms report that they go
  nowhere rather than pretending to succeed. Record them if you want them live.
