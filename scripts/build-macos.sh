#!/bin/bash
# Native macOS builds reuse the shared frontend, engine, protocols and storage.
set -eo pipefail
speak_task="${1:-Check}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"
speak_root="$(cd "$(dirname "$0")/.." && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then echo 'This entry point requires macOS.' >&2; exit 2; fi
case "$speak_task" in Check|Test|Package) ;; *) echo 'Usage: build-macos.sh [Check|Test|Package] [target]' >&2; exit 2;; esac
for speak_tool in cargo node npm xcrun; do command -v "$speak_tool" >/dev/null || { echo "Missing build tool: $speak_tool" >&2; exit 2; }; done
cd "$speak_root/app"
if [[ ! -d node_modules ]]; then npm ci --no-audit --no-fund; fi
npm run build
speak_target_args=()
if [[ -n "${2:-}" ]]; then speak_target_args=(--target "$2"); fi
case "$speak_task" in
 Check) cargo check --locked --manifest-path src-tauri/Cargo.toml --features desktop "${speak_target_args[@]}" ;;
 Test)
  cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop --lib "${speak_target_args[@]}" -- --test-threads=4
  cd "$speak_root"
  for speak_check in scripts/*-helper-check.mjs scripts/*-component-check.mjs scripts/live-level-check.mjs scripts/macos-ui-check.mjs; do node "$speak_check"; done
  ;;
 Package) node node_modules/@tauri-apps/cli/tauri.js build "${speak_target_args[@]}" ;;
esac
