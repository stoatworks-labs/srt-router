#!/usr/bin/env bash
#
# Publish a built directory to the repo's gh-pages branch.
#
# Vendored into each repo that has a Pages site. Deliberately does NOT use
# GitHub Actions: the org's Actions quota has run out before, and when it does,
# workflows fail in about three seconds in a way that looks like an outage
# rather than a quota. Building locally and pushing the result costs no minutes.
#
# One-time setup on GitHub:
#   Settings -> Pages -> Source: "Deploy from a branch" -> gh-pages / (root)
#
# Usage:
#   deploy-pages.sh --dist demo/dist [--label "flock demo"] [--dry-run]
set -euo pipefail

DIST="" LABEL="site" DRY_RUN=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dist) DIST="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=true; shift ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$DIST" ]] || { echo "error: --dist is required" >&2; exit 1; }

BRANCH=gh-pages
REMOTE=origin

git rev-parse --git-dir >/dev/null 2>&1 || { echo "error: not a git repository" >&2; exit 1; }
git remote get-url "$REMOTE" >/dev/null 2>&1 || { echo "error: no '$REMOTE' remote" >&2; exit 1; }

[[ -f "$DIST/index.html" ]] || { echo "error: $DIST/index.html missing — nothing to publish" >&2; exit 1; }

if [[ -n "$(git status --porcelain)" ]]; then
  echo "warning: working tree is dirty — publishing the built output anyway," >&2
  echo "         but the source commit won't match what goes live." >&2
fi

SOURCE_REF="$(git rev-parse --short HEAD)"

# GitHub Pages runs Jekyll by default, which silently drops any path starting
# with an underscore. The failure looks like a broken deploy, not a filter.
touch "$DIST/.nojekyll"

echo "==> Publishing $(find "$DIST" -type f | wc -l | tr -d ' ') files to $BRANCH"

if $DRY_RUN; then
  echo "--dry-run: would publish the contents of $DIST/ to $REMOTE/$BRANCH"
  find "$DIST" -maxdepth 2 -type f | sed 's|^|    |'
  exit 0
fi

WORKTREE="$(mktemp -d)"
cleanup() { git worktree remove --force "$WORKTREE" 2>/dev/null || true; }
trap cleanup EXIT

# A detached worktree keeps the publish completely off the current branch: no
# stashing, no checkout, and an interrupted run can't leave the source tree
# holding built output.
git fetch -q "$REMOTE" "$BRANCH" 2>/dev/null || true
if git show-ref --verify --quiet "refs/remotes/$REMOTE/$BRANCH"; then
  git worktree add --force "$WORKTREE" -B "$BRANCH" "$REMOTE/$BRANCH" >/dev/null
else
  git worktree add --force --detach "$WORKTREE" >/dev/null
  git -C "$WORKTREE" checkout --orphan "$BRANCH" >/dev/null 2>&1
  git -C "$WORKTREE" rm -rf . >/dev/null 2>&1 || true
fi

find "$WORKTREE" -mindepth 1 -maxdepth 1 -not -name .git -exec rm -rf {} +
cp -R "$DIST"/. "$WORKTREE"/

git -C "$WORKTREE" add -A
if git -C "$WORKTREE" diff --cached --quiet; then
  echo "==> No change since the last deploy. Nothing to push."
  exit 0
fi

git -C "$WORKTREE" commit -q -m "Deploy $LABEL from ${SOURCE_REF}"
git -C "$WORKTREE" push -q "$REMOTE" "$BRANCH"

REPO="$(basename -s .git "$(git remote get-url "$REMOTE")")"
echo "==> Published. Live shortly at https://stoatworks-labs.com/$REPO/"
