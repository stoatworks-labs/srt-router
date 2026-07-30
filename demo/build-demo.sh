#!/usr/bin/env bash
#
# Assemble a hosted demo from an app's own static assets plus the demo shim.
#
# Vendored into each repo that publishes one. It copies the app's real,
# unmodified UI, drops in demo-shim.js and the recorded fixtures, and injects
# the shim's <script> tag ahead of the app's own so the interception is in place
# before any request is made.
#
# Usage:
#   build-demo.sh --src <dir with index.html> --fixtures <demo-fixtures.json> \
#                 --out <output dir> [--base /repo-name/]
set -euo pipefail

SRC="" FIXTURES="" OUT="" BASE="/"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --src) SRC="$2"; shift 2 ;;
    --fixtures) FIXTURES="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --base) BASE="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

# Written for bash 3.2, which is what macOS ships — no ${var,,} or associative arrays.
[[ -n "$SRC" ]]      || { echo "error: --src is required" >&2; exit 1; }
[[ -n "$FIXTURES" ]] || { echo "error: --fixtures is required" >&2; exit 1; }
[[ -n "$OUT" ]]      || { echo "error: --out is required" >&2; exit 1; }
[[ -f "$SRC/index.html" ]] || { echo "error: no index.html in $SRC" >&2; exit 1; }
[[ -f "$FIXTURES" ]] || { echo "error: fixtures not found: $FIXTURES" >&2; exit 1; }

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$SRC"/. "$OUT"/
cp "$HERE/demo-shim.js" "$OUT/demo-shim.js"
cp "$FIXTURES" "$OUT/demo-fixtures.json"

# These apps are served from their backend's root, so their markup references
# /app.js and /style.css absolutely. That is already right for a Cloudflare
# Pages project (it serves at the root of its own domain); --base only matters
# when hosting under a subdirectory, where those paths need rewriting. Either
# way the shim has to load before the app's own script.
python3 - "$OUT/index.html" "$BASE" <<'PY'
import re, sys
path, base = sys.argv[1], sys.argv[2]
html = open(path, encoding='utf-8').read()

if base != '/':
    # base is '/repo/' — put the whole thing back, not just the tail, or the
    # rewritten path becomes relative and resolves under itself. Skip anything
    # already under the base: a bundler told to build for a subdirectory has
    # done this already, and rewriting again nests it under itself.
    def rebase(match):
        prefix, path = match.group(1), match.group(2)
        if path.startswith(base):
            return match.group(0)
        return prefix + base + path.lstrip('/')

    html = re.sub(r'((?:src|href)=")(/(?!/)[^"]*)', rebase, html)

# Served straight from disk, these files no longer get the backend's
# `Content-Type: text/html; charset=utf-8` header, so the encoding has to be
# declared in the markup or non-ASCII text renders as mojibake. Deliberately
# NOT adding a doctype: some of these documents are authored against quirks
# mode, and switching them to standards mode would change the layout the demo
# is supposed to be showing.
if not re.search(r'<meta\s+charset', html, re.I):
    html = '<meta charset="utf-8">\n' + html

shim = f'<script src="demo-shim.js" data-fixtures="demo-fixtures.json" data-base="{base}"></script>\n'
if 'demo-shim.js' not in html:
    # Before the first <script src=...>, or failing that at the end of <body>.
    match = re.search(r'<script\b[^>]*\bsrc=', html)
    if match:
        html = html[:match.start()] + shim + html[match.start():]
    else:
        html = html.replace('</body>', shim + '</body>')

open(path, 'w', encoding='utf-8').write(html)
print(f'  injected demo-shim.js, base={base}')
PY

echo "==> Demo built in $OUT ($(find "$OUT" -type f | wc -l | tr -d ' ') files)"
