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
#                 --out <output dir> [--base /repo-name/] [--version v1.2.3]
set -euo pipefail

SRC="" FIXTURES="" OUT="" BASE="/" VERSION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --src) SRC="$2"; shift 2 ;;
    --fixtures) FIXTURES="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --base) BASE="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
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

# The version the footer reports, for `data-version` below.
#
# Derived rather than required, because --version would otherwise have to be
# remembered on every direct re-run — and a direct re-run is the documented way
# to rebuild dist/ after a support-footer sync, so a forgotten flag would quietly
# strip the version off every report filed afterwards.
#
# This script is vendored into <repo>/demo/, so the repo root is one level up.
# Deliberately not `git describe`: dist/ is committed, and a build cannot know
# the sha of the commit that will contain it.
repo_version() {
  _root="$HERE/.."
  if [ -f "$_root/package.json" ]; then
    sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
      "$_root/package.json" | head -1
    return
  fi
  # [workspace.package] for these workspaces, [package] for a single crate;
  # scoped to that table so a dependency's version cannot be picked up instead.
  [ -f "$_root/Cargo.toml" ] || return 0
  awk '
    /^\[/ { inpkg = ($0 ~ /^\[(workspace\.)?package\]/) }
    inpkg && /^version[[:space:]]*=/ && match($0, /"[^"]+"/) {
      print substr($0, RSTART + 1, RLENGTH - 2); exit
    }
  ' "$_root/Cargo.toml"
}

if [ -z "$VERSION" ]; then
  _v="$(repo_version)"
  [ -n "$_v" ] && VERSION="v$_v"
fi
[ -n "$VERSION" ] || echo "  warning: no version found for the footer's data-version" >&2

rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$SRC"/. "$OUT"/
cp "$HERE/demo-shim.js" "$OUT/demo-shim.js"
cp "$FIXTURES" "$OUT/demo-fixtures.json"

# The support footer, if the repo has been synced with it. Optional on purpose:
# an older checkout without the file should still build a demo rather than fail.
# Its app name and repo URL are read out of the fixtures below, from the same
# meta the shim's banner uses, so the two can never name different projects.
if [ -f "$HERE/support-footer.js" ]; then
  cp "$HERE/support-footer.js" "$OUT/support-footer.js"
fi

# These apps are served from their backend's root, so their markup references
# /app.js and /style.css absolutely. That is already right for a Cloudflare
# Pages project (it serves at the root of its own domain); --base only matters
# when hosting under a subdirectory, where those paths need rewriting. Either
# way the shim has to load before the app's own script.
python3 - "$OUT/index.html" "$BASE" "$OUT" "$VERSION" <<'PY'
import json, os, re, sys
path, base, out, version = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
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
    # Before the app's first <script> of ANY kind, inline or external. An inline
    # script runs the moment the parser reaches it, so matching only `src=` put
    # the shim after it and the app called the real fetch() before window.fetch
    # was ever patched — its requests went to the static host and 404'd.
    # The </body> fallback is only correct for a document with no scripts at all.
    match = re.search(r'<script\b', html, re.I)
    if match:
        html = html[:match.start()] + shim + html[match.start():]
    elif '</head>' in html:
        html = html.replace('</head>', shim + '</head>')
    else:
        html = html.replace('</body>', shim + '</body>')

# The support footer goes LAST, after the app's own scripts — it is in-flow
# content appended to <body>, and nothing else waits on it. Relative src, like
# the shim's, so the --base rewrite above does not have to know about it.
#
# The app name and repo come from the fixtures' meta rather than new arguments,
# so the footer and the shim's banner always name the same project. No
# data-note: these demos are recorded against simulated devices, and every note
# worth writing ("nothing leaves your browser") is a claim about a real backend.
#
# data-version does NOT come from the meta, though: the fixtures are a recording
# and are re-made rarely, so a version baked into them would name whatever
# release the demo was last recorded against rather than the one this dist/ was
# built from. It comes from the repo's own manifest at build time instead.
footer_js = os.path.join(out, 'support-footer.js')
if os.path.exists(footer_js) and 'support-footer.js' not in html:
    meta = {}
    try:
        with open(os.path.join(out, 'demo-fixtures.json'), encoding='utf-8') as fh:
            meta = json.load(fh).get('meta', {}) or {}
    except (OSError, ValueError) as err:
        print(f'  warning: could not read fixtures meta for the footer: {err}')

    attrs = ['src="support-footer.js"', 'defer']
    for name, key in (('data-app', 'app'), ('data-repo', 'repo')):
        value = meta.get(key)
        if value:
            attrs.append(f'{name}="{value}"')
    if version:
        attrs.append(f'data-version="{version}"')
    footer = '<script ' + ' '.join(attrs) + '></script>\n'

    if '</body>' in html:
        html = html.replace('</body>', footer + '</body>')
    else:
        # Several of these documents have no <body> tag at all — they are
        # fragments the backend served with the right content type. Appending is
        # correct there: the parser puts it at the end of the implied body.
        html = html.rstrip() + '\n' + footer
    stamped = f' ({version})' if version else ''
    print(f'  injected support-footer.js for {meta.get("app", "the app")}{stamped}')

open(path, 'w', encoding='utf-8').write(html)
print(f'  injected demo-shim.js, base={base}')
PY

echo "==> Demo built in $OUT ($(find "$OUT" -type f | wc -l | tr -d ' ') files)"
