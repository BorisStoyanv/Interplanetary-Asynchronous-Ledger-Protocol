#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SOURCE_NODE_URL="${SOURCE_NODE_URL:-ws://127.0.0.1:9944}"
TARGET_NODE_URL="${TARGET_NODE_URL:-ws://127.0.0.1:9964}"
TARGET_SUBMITTER_SURI="${TARGET_SUBMITTER_SURI:-//Dave}"
RELAY_URL="http://127.0.0.1:9950"
TEMP_CONFIG="$(mktemp /tmp/ialp-transport-blackout.XXXXXX.toml)"
BLACKOUT_START="$(python3 - <<'PY'
from datetime import datetime, timezone, timedelta
print((datetime.now(timezone.utc)).isoformat().replace("+00:00", "Z"))
PY
)"
BLACKOUT_END="$(python3 - <<'PY'
from datetime import datetime, timezone, timedelta
print((datetime.now(timezone.utc) + timedelta(seconds=25)).isoformat().replace("+00:00", "Z"))
PY
)"

cat > "$TEMP_CONFIG" <<EOF
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
base_one_way_delay_seconds = 2
initial_retry_delay_seconds = 1
max_retry_delay_seconds = 30
max_attempts = 0

[[links.blackout_windows]]
start = "$BLACKOUT_START"
end = "$BLACKOUT_END"

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

cargo run -p ialp-summary-relay -- run --transport-config "$TEMP_CONFIG" > /tmp/ialp-relay-blackout.log 2>&1 &
sleep 2
cargo run -p ialp-summary-importer -- run \
  --domain mars \
  --node-url "$TARGET_NODE_URL" \
  --submitter-suri "$TARGET_SUBMITTER_SURI" \
  --transport-config "$TEMP_CONFIG" > /tmp/ialp-importer-blackout.log 2>&1 &
sleep 2
cargo run -p ialp-summary-exporter -- run \
  --domain earth \
  --node-url "$SOURCE_NODE_URL" \
  --relay-url "$RELAY_URL" \
  --transport-config "$TEMP_CONFIG" > /tmp/ialp-exporter-blackout.log 2>&1 &

SEEN_BLOCKED=0
for _ in $(seq 1 150); do
  STATUS="$(cargo run -p ialp-summary-relay -- status --transport-config "$TEMP_CONFIG" --target-domain mars --json)"
  if echo "$STATUS" | grep -q '"state": "blocked_by_blackout"'; then
    SEEN_BLOCKED=1
  fi
  if echo "$STATUS" | grep -q '"state": "importer_acked"'; then
    echo "$STATUS"
    if [[ "$SEEN_BLOCKED" -eq 1 ]]; then
      exit 0
    fi
    echo "delivery completed without ever entering blocked_by_blackout"
    exit 1
  fi
  sleep 2
done

echo "timed out waiting for blackout deferral/resume"
tail -n 50 /tmp/ialp-relay-blackout.log || true
tail -n 50 /tmp/ialp-importer-blackout.log || true
tail -n 50 /tmp/ialp-exporter-blackout.log || true
exit 1
