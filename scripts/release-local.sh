#!/usr/bin/env bash
# release-local.sh — cut a full SRT Router release from this Mac.
#
# GitHub Actions minutes are exhausted, so releases are built here. The heavy
# lifting lives in scripts/release-rust.sh (shared across the fleet); this file
# only says what SRT Router is.
#
#   scripts/release-local.sh                  build into dist-release/
#   scripts/release-local.sh --version 0.2.0  set an explicit version
#   scripts/release-local.sh --no-vm          skip the Windows launcher bundles
#   scripts/release-local.sh --upload         tag and publish the GitHub release
set -euo pipefail

RR_NAME="SRT Router"
RR_SLUG="srt-router"
RR_IDENT="com.stoatworks.srt-router"
RR_EXTRA_FILES=("README.md" "LICENSE")
RR_EXTRA_DIRS=("config")
RR_LAUNCHER="launcher"
RR_APP_NAME="SRT Router.app"
RR_SERVER_BIN="srtrouter"

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release-rust.sh"
