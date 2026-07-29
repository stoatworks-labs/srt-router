# AGENTS.md — bringing an LLM up to speed on srt-router

Orientation for an AI assistant (or a new human) picking this project up cold. `CLAUDE.md`
holds the short command reference; this file explains the model and the traps.

---

## 1. What this is

A **crosspoint-based SRT router**, in Rust: any number of SRT inputs, any number of SRT
outputs, and a router-style crosspoint connecting them — each output picks exactly one
source, switchable live.

The mental model is deliberately **a broadcast video router applied to SRT streams instead of
SDI/HDMI**. Sources × destinations matrix. If a proposed feature doesn't fit that model, it
probably belongs somewhere else.

Public repo, ships a user-facing AI-assisted disclaimer.

## 2. Layout

```
crates/
  core       Crosspoint / matrix model and shared types. The I/O trait lives here.
  router     Main binary: relay + orchestration
  srt-io     SRT backend
  ndi-io     NDI backend
  omt-io     OMT backend  (see the build trap below)
  media-io   Media file I/O
  web        Web UI
docs/architecture.md, roadmap.md, diagrams/
```

**Architectural rule: keep every new I/O backend behind `core`'s I/O trait.** The whole point
of the crosspoint model is that the matrix doesn't know or care what a source is. A backend
that reaches into `router` directly breaks that.

## 3. Build — the trap you will hit first

```bash
cargo build            # workspace
cargo test
cargo run -p router
```

**`cargo check`/`build` on the full workspace fails out of the box** with:

```
error: failed to run custom build command for `omt-io`
  OMT_LIB_DIR is not set. Point it at the directory containing libomt's
  dynamic library, extracted from a libomtnet release zip's Libraries/<platform> folder
```

This is **not a bug and not a broken repo.** `omt-io` links an external library that isn't
vendored. Either set `OMT_LIB_DIR` to an extracted
[libomtnet](https://github.com/openmediatransport/libomtnet/releases) release, or work on a
subset:

```bash
cargo build -p core -p router -p srt-io       # skip omt-io
```

The build script's error message is clear; the repo docs did not previously mention it, which
is why a newcomer reads a normal external dependency as a broken checkout.

`ndi-io` has the same shape of dependency — the NDI SDK must be installed for it to compile.

## 4. Linting

```bash
cargo clippy --all-targets --all-features
```

Note this differs from the sibling openstage project, where `--all-features` is explicitly
forbidden for the same NDI reason. Here it is the documented command — but it will still
require the NDI/OMT dependencies present. On a machine without them, lint per-crate.

## 5. Status — what's actually proven

Phase 1 (relay + web UI) is **built and verified locally**, including integration tests that
relay real SRT and NDI protocol traffic end-to-end, and a live crosspoint switch driven from
the web UI.

What has **not** happened, and shouldn't be implied:
- Never run against **real third-party SRT/NDI encoders or decoders**.
- Never run over an **actual non-loopback network path**.

Everything proven so far is loopback and synthetic. That's a real result, but a live
broadcast chain is a different claim.

## 6. Conventions

- Multi-platform release CI. Cross-compile macOS x86_64 on `macos-14` — **never `macos-13`**,
  those Intel runners are retired.
- Ships as its own desktop app via **av-launcher** (Tauri v2 tray shell, server embedded).
  Note the macOS Gatekeeper trap that affects every av-launcher app: for an unsigned `.app`
  bundling helper binaries, approving the app does **not** unquarantine its payload — the
  helpers get SIGKILLed silently.
- Public repo. "Commit" means commit **and** push.

## Diagnostics

Log via `tracing` as usual; `crates/diag` adds a rotating file, an in-memory ring and a
panic hook that writes a JSON crash report. Wire it as the **first** thing in `main`, and
**hold the returned guard** — dropping it (`let _ = diag::init(..)`) silently stops the log
file being written. Console output goes to stderr; stdout is reserved for program output.
See [docs/diagnostics.md](docs/diagnostics.md).
