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

# Ship the NDI runtime inside the installers. Permitted — the SDK is
# royalty-free — provided the licence we distribute under forbids modifying and
# reverse-engineering it, which rl_eula adds to the installer's EULA. The source
# tree still carries no NDI binaries: MIT cannot impose that condition.
#
# Targets whose runtime is not on this host are skipped, not failed. Set
# RL_NDI_DIR_LINUX_X86_64 / RL_NDI_DIR_WINDOWS_X86_64 (and the aarch64 variants)
# to bundle those too; macOS is found automatically from the installed SDK.
#
# Obligation this creates: keep the bundled runtime current. Nothing checks it.
RR_BUNDLE_NDI=1

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release-rust.sh"
