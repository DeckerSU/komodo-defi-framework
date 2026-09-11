#!/usr/bin/env bash
set -euo pipefail
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
workspace_dir=$(CDPATH= cd -- "$repo_dir/.." && pwd)
coins_dir=${KDF_TON_COINS_DIR:-"$workspace_dir/coins"}
runtime_dir=${KDF_TON_RUNTIME_DIR:-"$workspace_dir/integration-runs/ton-gram"}
kdf_binary=${KDF_TON_BINARY:-}

command -v sha256sum >/dev/null
test -f "$coins_dir/coins"
test -f "$coins_dir/ton/GRAM"

mkdir -p "$runtime_dir"/{config,db/hd,db/iguana,logs,results}
if [[ -z "$kdf_binary" ]]; then
  command -v cargo >/dev/null
  cargo +1.90.0 build --release --offline -p mm2_bin_lib --bin kdf
  kdf_binary="$repo_dir/target/release/kdf"
fi
test -x "$kdf_binary"
install -m 0700 "$kdf_binary" "$runtime_dir/kdf"
install -m 0600 "$coins_dir/coins" "$runtime_dir/coins"
mkdir -p "$runtime_dir/ton"
install -m 0600 "$coins_dir/ton/GRAM" "$runtime_dir/ton/GRAM"
install -m 0700 "$script_dir/start-kdf.sh" "$runtime_dir/start-kdf.sh"
install -m 0700 "$script_dir/test-rpc.sh" "$runtime_dir/test-rpc.sh"
install -m 0700 "$script_dir/generate-config.py" "$runtime_dir/generate-config.py"

kdf_revision=$(git -C "$repo_dir" rev-parse HEAD)
coins_revision=$(git -C "$coins_dir" rev-parse HEAD)
python3 - "$runtime_dir/manifest.json" "$kdf_revision" "$coins_revision" "$runtime_dir" <<'PY'
import hashlib, json, pathlib, subprocess, sys
manifest, kdf_revision, coins_revision, runtime = sys.argv[1:]
runtime = pathlib.Path(runtime)
files = ["kdf", "coins", "ton/GRAM", "start-kdf.sh", "test-rpc.sh", "generate-config.py"]
hashes = {name: hashlib.sha256((runtime / name).read_bytes()).hexdigest() for name in files}
pathlib.Path(manifest).write_text(json.dumps({
    "kdf_revision": kdf_revision,
    "coins_revision": coins_revision,
    "rust_toolchain": "1.90.0",
    "sha256": hashes,
}, indent=2) + "\n")
PY
chmod 600 "$runtime_dir/manifest.json"
printf 'Prepared TON runtime at %s\n' "$runtime_dir"
