#!/usr/bin/env bash
set -euo pipefail
umask 077

mode=${1:-hd}
send=${2:-}
case "$mode" in hd|iguana) ;; *) echo "usage: $0 [hd|iguana] [--send]" >&2; exit 2;; esac
case "$send" in ''|--send) ;; *) echo "unknown option: $send" >&2; exit 2;; esac
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
port=${KDF_TON_RPC_PORT:-$([[ "$mode" == hd ]] && echo 17783 || echo 17784)}
config="$script_dir/config/$mode.json"
test -r "$config"
command -v curl >/dev/null
command -v jq >/dev/null
userpass=$(jq -r '.rpc_password' "$config")
rpc_url="http://127.0.0.1:$port"

rpc() {
  curl --fail --silent --show-error --connect-timeout 5 --max-time 30 --url "$rpc_url" --data @-
}
assert_json() { jq -e "$1" >/dev/null; }

enable=$(rpc <<JSON
{"userpass":"$userpass","method":"enable","coin":"GRAM","nodes":[{"url":"https://toncenter.com/api/v2"}],"tx_history":true}
JSON
)
printf '%s' "$enable" | assert_json '.result.address | type == "string"'
address=$(printf '%s' "$enable" | jq -r '.result.address')

balance=$(rpc <<JSON
{"userpass":"$userpass","method":"my_balance","coin":"GRAM"}
JSON
)
printf '%s' "$balance" | assert_json '.balance | tonumber >= 0'

legacy_history=$(rpc <<JSON
{"userpass":"$userpass","method":"my_tx_history","coin":"GRAM","limit":10}
JSON
)
printf '%s' "$legacy_history" | assert_json '.result.transactions | type == "array"'

v2_history=$(rpc <<JSON
{"mmrpc":"2.0","userpass":"$userpass","method":"my_tx_history","params":{"coin":"GRAM","limit":10,"target":{"type":"iguana"}}}
JSON
)
printf '%s' "$v2_history" | assert_json '.result.transactions | type == "array"'

unsigned_withdraw=$(rpc <<JSON
{"userpass":"$userpass","method":"withdraw","coin":"GRAM","to":"$address","amount":"0.001","broadcast":false,"expiration_seconds":60}
JSON
)
printf '%s' "$unsigned_withdraw" | assert_json '.tx_hex | type == "string"'

if [[ "$send" == --send ]]; then
  tx_hex=$(printf '%s' "$unsigned_withdraw" | jq -r '.tx_hex')
  broadcast=$(rpc <<JSON
{"userpass":"$userpass","method":"send_raw_transaction","coin":"GRAM","tx_hex":"$tx_hex"}
JSON
)
  printf '%s' "$broadcast" | assert_json '.tx_hash | type == "string"'
fi

mkdir -p "$script_dir/results"
printf '{"mode":"%s","address":"%s","send":%s}\n' "$mode" "$address" "$([[ "$send" == --send ]] && echo true || echo false)" >"$script_dir/results/$mode.json"
printf 'TON RPC checks passed for %s address %s\n' "$mode" "$address"
