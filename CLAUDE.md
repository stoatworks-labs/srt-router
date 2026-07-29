# srt-router

Crosspoint-based SRT router (Rust). Relays/routes SRT streams through a matrix, with NDI/OMT/media I/O backends and a web UI. Phase 1 (relay + web UI) built & verified.

## Commands
- Build: `cargo build` (workspace)
- Test: `cargo test`
- Lint: `cargo clippy --all-targets --all-features`
- Run: `cargo run -p router`

## Layout (crates/)
- `core` — crosspoint/matrix model & shared types
- `router` — main binary (relay + orchestration)
- `srt-io` / `ndi-io` / `omt-io` / `media-io` — protocol/media I/O backends
- `web` — web UI

## Notes
- Routing is crosspoint-based: sources × destinations matrix — keep new I/O backends behind the `core` I/O trait.
- Multi-platform release CI; cross-compile macOS x86_64 on macos-14 (never macos-13).
- Public repo. Ships user-facing AI disclaimer. "Commit" = commit **and** push.

## Diagnostics

Log via `tracing` as usual; `crates/diag` adds a rotating file, an in-memory ring and a
panic hook that writes a JSON crash report. Wire it as the **first** thing in `main`, and
**hold the returned guard** — dropping it (`let _ = diag::init(..)`) silently stops the log
file being written. Console output goes to stderr; stdout is reserved for program output.
See [docs/diagnostics.md](docs/diagnostics.md).
