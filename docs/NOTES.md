# Notes

Working notes for this repo: status, decisions, and the traps that have actually bitten.
Migrated out of Claude Code's memory on 2026-08-24, so they are written in the first
person and dated by when each thing was learned — that date is usually the useful part.

Cross-cutting notes that are not specific to this repo live in
[fleet-notes](https://github.com/stoatworks-labs/fleet-notes).

*srt-router — crosspoint-based SRT stream router (Rust), local repo at ~/Projects/srt-router, GitHub public repo.*

Crosspoint-based [SRT](https://www.srtalliance.org/) router: any number of
SRT inputs/outputs connected through a router-style crosspoint (each output
picks exactly one source, switchable live) — same mental model as a
broadcast video router (like [av mainframe](https://github.com/stoatworks-labs/av-mainframe/blob/main/docs/NOTES.md) (`av-mainframe`)) but for SRT streams
over IP instead of raw HDMI/SDI pixels. Repo at `~/Projects/srt-router`,
pushed public to GitHub `allansargeant/srt-router` on 2026-07-15, branch
`main`.

**Scope decided by user (2026-07-15):** mostly a pure relay (no
decode/re-encode, switching is a channel resubscription not a transcode —
cheap, but not pixel-seamless, same tradeoff real SRT/IP routers like
Zixi/Haivision accept), but with the core engine designed so future
special-purpose sources (scalers, a local media player, stills) can
register into the same crosspoint later without changing the core engine —
see "extension point" design in docs/architecture.md. Stack: Rust
(consistent with [dante babelbox](https://github.com/stoatworks-labs/Dante-BabelBox/blob/main/docs/NOTES.md) (`Dante-BabelBox`)), built on
[srt-tokio](https://github.com/russelltg/srt-rs) (pure-Rust SRT, no libsrt/C
dependency — verified via web research this is a real, actively-published
crate, confirmed its actual API from the repo's own example files rather
than guessing). Control: local web UI (crosspoint grid, click-to-route)
backed by a REST API; external API/Companion integration deferred to a
later phase per user's explicit answer.

**Phase 1 status: built and verified working, not yet field-tested.**
Cargo workspace, 4 crates: `crosspoint-core` (transport-agnostic
source/output/crosspoint engine — broadcast channel per source, watch
channel per output, unit tested), `srt-io` (SRT listener/caller
input/output wrapping srt-tokio, auto-reconnect), `crosspoint-web` (axum
REST API + embedded single-page HTML/JS grid UI), `srtrouter` (bin, TOML
config). Verified for real, not hand-waved: `cargo test` passes, `cargo
clippy` clean, running the binary against `config/example.toml` actually
binds real SRT/UDP listener sockets (confirmed via `lsof`), the REST API
drives real crosspoint state changes (confirmed via `curl`), and clicking a
grid cell in an actual browser (Claude_Browser pane) correctly re-routes an
output end-to-end. **Not yet done:** no test against a real SRT
encoder/decoder or real network path (only local/loopback-adjacent so far),
no persistence of routing across restart, no Phase 2 special-purpose
sources, no auth on the web UI. Full phased plan in
`~/Projects/srt-router/docs/roadmap.md`.

One tooling gotcha worth remembering for future browser-driven UI testing
in this environment: the Claude_Browser `computer` tool's screenshot-pixel
coordinates did not reliably map to actual page click targets in this
session (device pixel ratio / viewport-vs-screenshot scaling mismatch —
screenshot reported 800px wide, actual viewport was 1038 CSS px at DPR 2)
— clicks at seemingly-correct coordinates silently landed on nothing.
Diagnosed by cross-checking with `curl` against the REST API (state didn't
change) and confirming via `read_network_requests` that no POST fired, then
isolating the automation-layer bug from app logic by dispatching
`element.click()` directly via `javascript_tool` (which worked correctly).
If a browser-driven click ever appears to silently no-op again, verify with
network/API-level evidence before assuming the app is broken — the
automation coordinate mapping is a plausible false lead.

**Why:** Tracking this so future sessions don't need to re-derive the
project's scope, architecture, or verification status from scratch.

**How to apply:** Before recommending or building on specifics, re-read
`README.md`, `docs/architecture.md`, and `docs/roadmap.md` in the repo since
they may have evolved past this snapshot. **commit means push** (working-practice note, kept in Claude memory)
applies here too.
