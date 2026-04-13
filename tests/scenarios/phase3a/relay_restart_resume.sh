#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SOURCE_NODE_URL="${SOURCE_NODE_URL:-ws://127.0.0.1:9944}"
TARGET_NODE_URL="${TARGET_NODE_URL:-ws://127.0.0.1:9954}"
TARGET_SUBMITTER_SURI="${TARGET_SUBMITTER_SURI:-//Charlie}"
TEMP_CONFIG="$(mktemp /tmp/ialp-transport-restart.XXXXXX.toml)"
RELAY_URL="http://127.0.0.1:9950"

cat > "$TEMP_CONFIG" <<'EOF'
[relay]
listen_addr = "127.0.0.1:9950"
store_dir = "var/relay"
scheduler_tick_millis = 500
ack_poll_millis = 500

[importers.earth]
listen_addr = "127.0.0.1:9951"

[importers.moon]
listen_addr = "127.0.0.1:9952"

[importers.mars]
listen_addr = "127.0.0.1:9953"

[[links]]
source_domain = "earth"
target_domain = "moon"
base_one_way_delay_seconds = 12
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0
blackout_windows = []

[[links]]
source_domain = "earth"
target_domain = "mars"
base_one_way_delay_seconds = 4
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0
blackout_windows = []

[[links]]
source_domain = "moon"
target_domain = "earth"
base_one_way_delay_seconds = 2
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0
blackout_windows = []

[[links]]
source_domain = "moon"
target_domain = "mars"
base_one_way_delay_seconds = 3
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0
blackout_windows = []

[[links]]
source_domain = "mars"
target_domain = "earth"
base_one_way_delay_seconds = 4
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0
blackout_windows = []

[[links]]
source_domain = "mars"
target_domain = "moon"
base_one_way_delay_seconds = 3
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0
blackout_windows = []
EOF

cleanup() {
  jobs -pr | xargs -r kill >/dev/null 2>&1 || true
  rm -f "$TEMP_CONFIG"
}
trap cleanup EXIT

cargo run -p ialp-summary-importer -- run \
  --domain moon \
  --node-url "$TARGET_NODE_URL" \
  --submitter-suri "$TARGET_SUBMITTER_SURI" \
  --transport-config "$TEMP_CONFIG" > /tmp/ialp-importer-restart.log 2>&1 &
sleep 2

cargo run -p ialp-summary-relay -- run --transport-config "$TEMP_CONFIG" > /tmp/ialp-relay-restart.log 2>&1 &
RELAY_PID=$!
sleep 2

cargo run -p ialp-summary-exporter -- run \
  --domain earth \
  --node-url "$SOURCE_NODE_URL" \
  --relay-url "$RELAY_URL" \
  --transport-config "$TEMP_CONFIG" > /tmp/ialp-exporter-restart.log 2>&1 &

for _ in $(seq 1 45); do
  STATUS="$(cargo run -p ialp-summary-relay -- status --transport-config "$TEMP_CONFIG" --target-domain moon --json)"
  if echo "$STATUS" | grep -q '"state": "scheduled"'; then
    kill "$RELAY_PID" || true
    wait "$RELAY_PID" || true
    sleep 2
    cargo run -p ialp-summary-relay -- run --transport-config "$TEMP_CONFIG" > /tmp/ialp-relay-restart-2.log 2>&1 &
    break
  fi
  sleep 2
done

for _ in $(seq 1 120); do
  STATUS="$(cargo run -p ialp-summary-relay -- status --transport-config "$TEMP_CONFIG" --target-domain moon --json)"
  if echo "$STATUS" | grep -q '"state": "importer_acked"'; then
    echo "$STATUS"
    exit 0
  fi
  sleep 2
done

echo "timed out waiting for relay restart/resume delivery"
tail -n 50 /tmp/ialp-relay-restart.log || true
tail -n 50 /tmp/ialp-relay-restart-2.log || true
tail -n 50 /tmp/ialp-importer-restart.log || true
tail -n 50 /tmp/ialp-exporter-restart.log || true
exit 1
