#!/usr/bin/env bash
set -euo pipefail
umask 077

mode=${1:-hd}
send=${2:-}
activation_api=${3:-legacy}
case "$mode" in hd|iguana) ;; *) echo "usage: $0 [hd|iguana] [--send] [legacy|v2]" >&2; exit 2;; esac
case "$send" in ''|--send) ;; *) echo "unknown option: $send" >&2; exit 2;; esac
case "$activation_api" in legacy|v2) ;; *) echo "unknown activation API: $activation_api" >&2; exit 2;; esac
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
port=${KDF_TON_RPC_PORT:-$([[ "$mode" == hd ]] && echo 17783 || echo 17784)}
config="$script_dir/config/$mode.json"
test -r "$config"
command -v curl >/dev/null
command -v jq >/dev/null
userpass=$(jq -r '.rpc_password' "$config")
rpc_url="http://127.0.0.1:$port"
ton_node=${KDF_TON_RPC_NODE:-https://toncenter.com/api/v2}
ton_api_key=${KDF_TON_API_KEY:-}
nodes=$(jq -cn --arg url "$ton_node" --arg api_key "$ton_api_key" \
  'if $api_key == "" then [{url: $url}] else [{url: $url, api_key: $api_key}] end')

rpc() {
  curl --fail --silent --show-error --connect-timeout 5 --max-time 30 --url "$rpc_url" --data @-
}
rpc_allow_error() {
  curl --silent --show-error --connect-timeout 5 --max-time 30 --url "$rpc_url" --data @-
}
assert_json() { jq -e "$1" >/dev/null; }

if [[ "$activation_api" == legacy ]]; then
enable=$(rpc <<JSON
{"userpass":"$userpass","method":"enable","coin":"GRAM","nodes":$nodes,"tx_history":true}
JSON
)
printf '%s' "$enable" | assert_json '.address | type == "string"'
address=$(printf '%s' "$enable" | jq -r '.address')
else
  init=$(rpc <<JSON
{"mmrpc":"2.0","userpass":"$userpass","method":"enable_ton","params":{"ticker":"GRAM","activation_params":{"nodes":$nodes,"tx_history":true}}}
JSON
)
  printf '%s' "$init" | assert_json '.result.task_id | type == "number"'
  task_id=$(printf '%s' "$init" | jq -r '.result.task_id')
  status=''
  for _ in $(seq 1 30); do
    status=$(rpc <<JSON
{"mmrpc":"2.0","userpass":"$userpass","method":"task::enable_ton::status","params":{"task_id":$task_id,"forget_if_finished":false}}
JSON
)
    printf '%s' "$status" | assert_json '.result.status | type == "string"'
    case "$(printf '%s' "$status" | jq -r '.result.status')" in
      Ok) break ;;
      Error) printf '%s\n' "$status" >&2; exit 1 ;;
      *) sleep 1 ;;
    esac
  done
  printf '%s' "$status" | assert_json '.result.status == "Ok"'
  address=$(printf '%s' "$status" | jq -r '.result.details.address')
fi
if [[ "$mode" == hd ]]; then
  test "$address" = "UQCLuOL1GAZuZbbhocUlGI3gxasW9HNK8zZpN7L-noXSF9l0"
fi

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

history_target='{"type":"iguana"}'
if [[ "$mode" == hd ]]; then
  history_target='{"type":"account_id","account_id":0}'
fi
v2_history=$(rpc <<JSON
{"mmrpc":"2.0","userpass":"$userpass","method":"my_tx_history","params":{"coin":"GRAM","limit":10,"target":$history_target}}
JSON
)
printf '%s' "$v2_history" | assert_json '.result.transactions | type == "array"'

if printf '%s' "$balance" | jq -e '.balance | tonumber >= 0.001' >/dev/null; then
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
else
  insufficient_balance=$(rpc_allow_error <<JSON
{"userpass":"$userpass","method":"withdraw","coin":"GRAM","to":"$address","amount":"0.001","broadcast":false,"expiration_seconds":60}
JSON
)
  printf '%s' "$insufficient_balance" | assert_json '.error | type == "string"'
  if [[ "$send" == --send ]]; then
    echo "cannot use --send with an unfunded GRAM address" >&2
    exit 1
  fi
fi

mkdir -p "$script_dir/results"
printf '{"mode":"%s","activation_api":"%s","address":"%s","send":%s}\n' "$mode" "$activation_api" "$address" "$([[ "$send" == --send ]] && echo true || echo false)" >"$script_dir/results/$mode-$activation_api.json"
printf 'TON RPC checks passed for %s address %s via %s activation\n' "$mode" "$address" "$activation_api"
