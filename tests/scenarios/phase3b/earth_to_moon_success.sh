#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
HARNESS_BIN="$ROOT/target/debug/ialp-scenario-harness"
NODE_BIN="$ROOT/target/debug/ialp-node"
EXPORTER_BIN="$ROOT/target/debug/ialp-summary-exporter"
RELAY_BIN="$ROOT/target/debug/ialp-summary-relay"
IMPORTER_BIN="$ROOT/target/debug/ialp-summary-importer"

for TOOLCHAIN_BIN in "$HOME"/.rustup/toolchains/stable-*/bin; do
  if [[ -d "$TOOLCHAIN_BIN" ]]; then
    export PATH="$TOOLCHAIN_BIN:$PATH"
    break
  fi
done

if [[ ! -x "$HARNESS_BIN" || ! -x "$NODE_BIN" || ! -x "$EXPORTER_BIN" || ! -x "$RELAY_BIN" || ! -x "$IMPORTER_BIN" ]]; then
  cargo build --locked \
    -p ialp-node \
    -p ialp-summary-exporter \
    -p ialp-summary-relay \
    -p ialp-summary-importer \
    -p ialp-scenario-harness
fi

declare -a ARGS=()
if [[ -n "${ARTIFACTS_DIR:-}" ]]; then
  ARGS+=(--artifacts-dir "$ARTIFACTS_DIR")
fi

if (( ${#ARGS[@]} > 0 )); then
  "$HARNESS_BIN" run --scenario earth-to-moon-success "${ARGS[@]}" --json
else
  "$HARNESS_BIN" run --scenario earth-to-moon-success --json
fi
