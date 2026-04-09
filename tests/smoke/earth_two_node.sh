#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

BIN="${ROOT_DIR}/target/release/ialp-node"
AUTH_LOG="$(mktemp -t ialp-earth-auth.XXXXXX.log)"
FOLLOWER_LOG="$(mktemp -t ialp-earth-follower.XXXXXX.log)"
AUTH_DIR="$(mktemp -d -t ialp-earth-auth.XXXXXX)"
FOLLOWER_DIR="$(mktemp -d -t ialp-earth-follow.XXXXXX)"

cleanup() {
  if [[ -n "${AUTH_PID:-}" ]]; then
    kill "${AUTH_PID}" >/dev/null 2>&1 || true
    wait "${AUTH_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${FOLLOWER_PID:-}" ]]; then
    kill "${FOLLOWER_PID}" >/dev/null 2>&1 || true
    wait "${FOLLOWER_PID}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if [[ ! -x "$BIN" ]]; then
  cargo build -p ialp-node --release --locked
fi

AUTH_KEY_OUTPUT="$("$BIN" key generate-node-key --chain earth --base-path "$AUTH_DIR" 2>&1 >/dev/null)"
FOLLOWER_KEY_OUTPUT="$("$BIN" key generate-node-key --chain earth --base-path "$FOLLOWER_DIR" 2>&1 >/dev/null)"
AUTH_PEER_ID="$(printf '%s\n' "$AUTH_KEY_OUTPUT" | sed -n 's/.*\(12D3[^ ]*\).*/\1/p' | tail -n 1)"

if [[ -z "$AUTH_PEER_ID" ]]; then
  echo "$AUTH_KEY_OUTPUT"
  echo "failed to derive authority peer id from generated node key" >&2
  exit 1
fi

"$BIN" --domain earth --base-path "$AUTH_DIR" --validator --alice >"$AUTH_LOG" 2>&1 &
AUTH_PID=$!
BOOTNODE="/ip4/127.0.0.1/tcp/30333/p2p/${AUTH_PEER_ID}"

"$BIN" \
  --domain earth \
  --base-path "$FOLLOWER_DIR" \
  --port 30334 \
  --rpc-port 9945 \
  --prometheus-port 9616 \
  --bootnodes "$BOOTNODE" >"$FOLLOWER_LOG" 2>&1 &
FOLLOWER_PID=$!

json_rpc() {
  local port="$1"
  local body="$2"
  curl -sS -H 'Content-Type: application/json' --data "$body" "http://127.0.0.1:${port}"
}

wait_for_rpc() {
  local port="$1"
  for _ in $(seq 1 60); do
    if json_rpc "$port" '{"jsonrpc":"2.0","id":1,"method":"system_health","params":[]}' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

if ! wait_for_rpc 9944; then
  cat "$AUTH_LOG"
  echo "authority RPC did not become ready" >&2
  exit 1
fi

if ! wait_for_rpc 9945; then
  cat "$FOLLOWER_LOG"
  echo "follower RPC did not become ready" >&2
  exit 1
fi

hex_to_dec() {
  local value="$1"
  echo $((16#${value#0x}))
}

for _ in $(seq 1 90); do
  AUTH_HEAD="$(json_rpc 9944 '{"jsonrpc":"2.0","id":1,"method":"chain_getFinalizedHead","params":[]}' | sed -n 's/.*"result":"\([^"]*\)".*/\1/p')"
  FOLLOWER_HEAD="$(json_rpc 9945 '{"jsonrpc":"2.0","id":1,"method":"chain_getFinalizedHead","params":[]}' | sed -n 's/.*"result":"\([^"]*\)".*/\1/p')"

  if [[ -n "$AUTH_HEAD" && -n "$FOLLOWER_HEAD" ]]; then
    AUTH_HEADER="$(json_rpc 9944 "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"chain_getHeader\",\"params\":[\"${AUTH_HEAD}\"]}")"
    FOLLOWER_HEADER="$(json_rpc 9945 "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"chain_getHeader\",\"params\":[\"${FOLLOWER_HEAD}\"]}")"
    AUTH_NUMBER_HEX="$(echo "$AUTH_HEADER" | sed -n 's/.*"number":"\([^"]*\)".*/\1/p')"
    FOLLOWER_NUMBER_HEX="$(echo "$FOLLOWER_HEADER" | sed -n 's/.*"number":"\([^"]*\)".*/\1/p')"

    if [[ -n "$AUTH_NUMBER_HEX" && -n "$FOLLOWER_NUMBER_HEX" ]]; then
      AUTH_NUMBER="$(hex_to_dec "$AUTH_NUMBER_HEX")"
      FOLLOWER_NUMBER="$(hex_to_dec "$FOLLOWER_NUMBER_HEX")"
      if [[ "$AUTH_HEAD" == "$FOLLOWER_HEAD" && "$AUTH_NUMBER" -ge 2 && "$FOLLOWER_NUMBER" -ge 2 ]]; then
        echo "Earth authority and follower converged on finalized head ${AUTH_HEAD}"
        exit 0
      fi
    fi
  fi

  sleep 2
done

cat "$AUTH_LOG"
cat "$FOLLOWER_LOG"
echo "nodes failed to converge on the same finalized head" >&2
exit 1
