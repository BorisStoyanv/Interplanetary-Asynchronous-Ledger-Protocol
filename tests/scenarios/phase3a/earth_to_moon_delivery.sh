#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TRANSPORT_CONFIG="${TRANSPORT_CONFIG:-$ROOT/config/transport/local.toml}"
SOURCE_NODE_URL="${SOURCE_NODE_URL:-ws://127.0.0.1:9944}"
TARGET_NODE_URL="${TARGET_NODE_URL:-ws://127.0.0.1:9954}"
TARGET_DOMAIN="moon"
TARGET_SUBMITTER_SURI="${TARGET_SUBMITTER_SURI:-//Charlie}"
RELAY_URL="${RELAY_URL:-http://127.0.0.1:9950}"

cleanup() {
  jobs -pr | xargs -r kill >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "starting relay"
cargo run -p ialp-summary-relay -- run --transport-config "$TRANSPORT_CONFIG" > /tmp/ialp-relay-moon.log 2>&1 &
sleep 2

echo "starting moon importer"
cargo run -p ialp-summary-importer -- run \
  --domain "$TARGET_DOMAIN" \
  --node-url "$TARGET_NODE_URL" \
  --submitter-suri "$TARGET_SUBMITTER_SURI" \
  --transport-config "$TRANSPORT_CONFIG" > /tmp/ialp-importer-moon.log 2>&1 &
sleep 2

echo "starting earth exporter"
cargo run -p ialp-summary-exporter -- run \
  --domain earth \
  --node-url "$SOURCE_NODE_URL" \
  --relay-url "$RELAY_URL" \
  --transport-config "$TRANSPORT_CONFIG" > /tmp/ialp-exporter-earth.log 2>&1 &

echo "polling relay/importer status until importer_acked"
for _ in $(seq 1 90); do
  STATUS="$(cargo run -p ialp-summary-relay -- status --transport-config "$TRANSPORT_CONFIG" --target-domain moon --json)"
  if echo "$STATUS" | grep -q '"state": "importer_acked"'; then
    echo "$STATUS"
    exit 0
  fi
  sleep 2
done

echo "timed out waiting for Earth -> Moon delivery"
tail -n 50 /tmp/ialp-relay-moon.log || true
tail -n 50 /tmp/ialp-importer-moon.log || true
tail -n 50 /tmp/ialp-exporter-earth.log || true
exit 1
