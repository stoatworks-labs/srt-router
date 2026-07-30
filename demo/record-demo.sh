#!/usr/bin/env bash
#
# Rebuild srt-router's hosted demo end to end.
#
# Runs the router with config/demo.toml (SRT only, loopback ports, no persisted
# state), records what it actually serves, then performs every crosspoint switch
# against it and records the router's real reaction to each. The web UI redraws
# purely from WebSocket pushes, so without those recordings the demo's grid
# would look interactive and do nothing.
#
# Re-run whenever the API or the UI changes, then publish with:
#   demo/deploy-pages.sh --dist demo/dist --label "srt-router demo"
set -euo pipefail

cd "$(dirname "$0")/.."

PORT=8098
BASE="http://127.0.0.1:$PORT"
SOURCES=(cam1 cam2 cam3 remote-feed)
OUTPUTS=(program preview stream-encoder)

echo "==> Building srtrouter"
cargo build -q --bin srtrouter

echo "==> Starting the router with the demo config"
cargo run -q --bin srtrouter -- --config config/demo.toml >/tmp/srt-router-demo-record.log 2>&1 &
ROUTER_PID=$!
cleanup() { kill "$ROUTER_PID" 2>/dev/null || true; }
trap cleanup EXIT

for _ in $(seq 1 60); do
  curl -sf "$BASE/api/state" >/dev/null 2>&1 && break
  sleep 1
done
curl -sf "$BASE/api/state" >/dev/null || {
  echo "error: router did not start; see /tmp/srt-router-demo-record.log" >&2; exit 1; }

# Every crosspoint the grid can make, so any click in the demo replays the
# router's own answer for that exact switch.
POST_ARGS=()
for output in "${OUTPUTS[@]}"; do
  for source in "${SOURCES[@]}"; do
    POST_ARGS+=(--post "/api/route|{\"output\":\"$output\",\"source\":\"$source\"}")
  done
done

echo "==> Recording (${#OUTPUTS[@]} outputs x ${#SOURCES[@]} sources = $(( ${#OUTPUTS[@]} * ${#SOURCES[@]} )) switches)"
node demo/record-fixtures.mjs \
  --base "$BASE" \
  --app "srt-router" --repo "https://github.com/stoatworks-labs/srt-router" \
  --get /api/state \
  --get /api/manage/sources \
  --get /api/manage/outputs \
  --get /api/manage/transports \
  --ws /ws --ws-seconds 10 \
  "${POST_ARGS[@]}" \
  --out demo/demo-fixtures.json

echo "==> Assembling the site"
demo/build-demo.sh \
  --src crates/web/static \
  --fixtures demo/demo-fixtures.json \
  --out demo/dist \
  --base /srt-router/

echo
echo "Preview it exactly as Pages will serve it:"
echo "  demo/serve-demo.py --dir demo/dist --base /srt-router/"
