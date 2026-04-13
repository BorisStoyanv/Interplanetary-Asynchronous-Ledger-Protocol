#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SOURCE_NODE_URL="${SOURCE_NODE_URL:-ws://127.0.0.1:9944}"
TARGET_NODE_URL="${TARGET_NODE_URL:-ws://127.0.0.1:9964}"
TARGET_SUBMITTER_SURI="${TARGET_SUBMITTER_SURI:-//Dave}"
TEMP_CONFIG="$(mktemp /tmp/ialp-transport-mars-delay.XXXXXX.toml)"
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
base_one_way_delay_seconds = 2
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0
blackout_windows = []

[[links]]
source_domain = "earth"
target_domain = "mars"
base_one_way_delay_seconds = 15
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

cargo run -p ialp-summary-relay -- run --transport-config "$TEMP_CONFIG" > /tmp/ialp-relay-mars.log 2>&1 &
sleep 2
cargo run -p ialp-summary-importer -- run \
  --domain mars \
  --node-url "$TARGET_NODE_URL" \
  --submitter-suri "$TARGET_SUBMITTER_SURI" \
  --transport-config "$TEMP_CONFIG" > /tmp/ialp-importer-mars.log 2>&1 &
sleep 2
cargo run -p ialp-summary-exporter -- run \
  --domain earth \
  --node-url "$SOURCE_NODE_URL" \
  --relay-url "$RELAY_URL" \
  --transport-config "$TEMP_CONFIG" > /tmp/ialp-exporter-earth-mars.log 2>&1 &

for _ in $(seq 1 120); do
  STATUS="$(cargo run -p ialp-summary-relay -- status --transport-config "$TEMP_CONFIG" --target-domain mars --json)"
  if echo "$STATUS" | grep -q '"state": "scheduled"'; then
    echo "$STATUS"
  fi
  if echo "$STATUS" | grep -q '"state": "importer_acked"'; then
    echo "$STATUS"
    exit 0
  fi
  sleep 2
done

echo "timed out waiting for Earth -> Mars delayed delivery"
tail -n 50 /tmp/ialp-relay-mars.log || true
tail -n 50 /tmp/ialp-importer-mars.log || true
tail -n 50 /tmp/ialp-exporter-earth-mars.log || true
exit 1
