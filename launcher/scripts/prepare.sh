#!/usr/bin/env bash
# Build the srtrouter release binary and copy it (plus its config template) into
# the launcher's bundled-resources dir. Run before `npm run tauri build`.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"      # launcher/
REPO="$(cd "$HERE/.." && pwd)"                # repo root

( cd "$REPO" && cargo build --release -p srtrouter )
mkdir -p "$HERE/src-tauri/bin"
cp "$REPO/target/release/srtrouter" "$HERE/src-tauri/bin/srtrouter"
cp "$REPO/config/example.toml"      "$HERE/src-tauri/bin/server-config.toml"
chmod +x "$HERE/src-tauri/bin/srtrouter"
echo "prepared src-tauri/bin/{srtrouter, server-config.toml}"
