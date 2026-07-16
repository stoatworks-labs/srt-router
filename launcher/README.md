# SRT Router Launcher

A Bitfocus Companion–style **tray launcher** for srt-router: pick a network
interface + port, Start/Stop the server, open the web UI, and run it from the
macOS menu bar. Built with [Tauri v2](https://tauri.app); the shipped `.app`
**bundles the `srtrouter` binary**, so it runs standalone.

![panel](docs/panel.png)

Download the latest `.dmg` from the repo's
[Releases](https://github.com/allansargeant/srt-router/releases).

> **Unsigned build.** On first launch macOS Gatekeeper will block it —
> right-click the app → **Open** → **Open**, once.

## What it does

- **GUI Interface** — every bindable IPv4 interface, plus "All interfaces (0.0.0.0)".
- **Port** — persisted between runs.
- **Start / Stop** — supervises the bundled `srtrouter` child process.
- **Launch GUI** — opens `http://<host>:<port>/` in your browser.
- **Hide** to the tray; **Quit** stops the server and exits.

Host:port is injected by patching srt-router's own config (`[web] bind`) and
passing the rendered file via `--config`. The launcher runs the server from its
writable app-config dir, where it also writes state.

## Building from source

The launcher bundles the release `srtrouter` binary. It's git-ignored (it lives
in the app's own build output and ships in the Release), so fetch it first:

```bash
cd launcher
./scripts/prepare.sh          # builds srtrouter --release and copies it into src-tauri/bin/
npm install
npm run tauri build           # -> src-tauri/target/release/bundle/{macos,dmg}/
```

Run in dev (uses the interface/port UI against the bundled binary):

```bash
npm run tauri dev
```

## How it relates to av-launcher

This is a self-contained copy of the reusable
[av-launcher](https://github.com/allansargeant/av-launcher) shell with
srt-router's config baked in. The Rust/JS shell is identical across the fleet;
only `src-tauri/launcher.toml`, the icon, and the bundled binary differ.
