#!/usr/bin/env bash
set -euo pipefail
umask 077

mode=${1:-hd}
case "$mode" in hd|iguana) ;; *) echo "usage: $0 [hd|iguana]" >&2; exit 2;; esac
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
runtime_dir=$script_dir
seed_file=${KDF_TON_SEED_FILE:-"$runtime_dir/../../seed-bip39-control.txt"}
port=${KDF_TON_RPC_PORT:-$([[ "$mode" == hd ]] && echo 17783 || echo 17784)}

test -x "$runtime_dir/kdf"
test -r "$seed_file"
test -r "$runtime_dir/seed-nodes.json"
mkdir -p "$runtime_dir/config" "$runtime_dir/db/$mode" "$runtime_dir/logs"
"$runtime_dir/generate-config.py" "$mode" "$seed_file" "$runtime_dir/seed-nodes.json" \
  "$runtime_dir/config/$mode.json" "$runtime_dir/db/$mode" "$port"

if [[ -f "$runtime_dir/$mode.pid" ]] && kill -0 "$(cat "$runtime_dir/$mode.pid")" 2>/dev/null; then
  echo "KDF is already running for $mode" >&2
  exit 1
fi
MM_CONF_PATH="$runtime_dir/config/$mode.json" MM_COINS_PATH="$runtime_dir/coins" MM_LOG="$runtime_dir/logs/$mode.log" \
  "$runtime_dir/kdf" >"$runtime_dir/logs/$mode.stdout.log" 2>&1 &
pid=$!
printf '%s\n' "$pid" >"$runtime_dir/$mode.pid"
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' INT TERM
for _ in $(seq 1 30); do
  if curl --silent --show-error --max-time 2 "http://127.0.0.1:$port" \
    --data '{"method":"version"}' >/dev/null 2>&1; then
    printf 'KDF %s is ready on 127.0.0.1:%s (pid %s)\n' "$mode" "$port" "$pid"
    wait "$pid"
    exit $?
  fi
  sleep 1
done
echo "KDF did not become ready; see $runtime_dir/logs/$mode.log" >&2
exit 1
