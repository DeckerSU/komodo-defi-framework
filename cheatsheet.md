# GRAM (TON) KDF cheat sheet

This sheet describes the native GRAM/TON integration on branch
`feat/ton-gram-integration`.  Examples use mainnet TON Center v2 and the packaged
runtime on `netid` 6133. They contain no wallet mnemonic, private key, API key, or
RPC password.

## Build and prepare an isolated runtime

KDF currently builds with Rust **1.90.0**. Build the binary, then package it together
with the GRAM-aware `coins` checkout and the runtime scripts:

```sh
cd /media/decker/data2tb/kdf-telegram/komodo-defi-framework-gram
cargo +1.90.0 build --release --offline -p mm2_bin_lib --bin kdf --target-dir target/ton-release
KDF_TON_BINARY="$PWD/target/ton-release/release/kdf" ./scripts/ton/prepare-runtime.sh
```

The package is created at `../integration-runs/ton-gram`. It has a copied KDF binary,
`coins` data containing GRAM, generated private runtime configuration, and launch and
test scripts. Re-run `prepare-runtime.sh` after changing KDF source or `../coins`.

Set the provider before activating the coin. A public TON Center endpoint is usable
without a key, but KDF deliberately limits anonymous requests to one per second. An
API key removes that local limiter and is never written to the generated configuration.

```sh
export KDF_TON_RPC_NODE='https://toncenter.com/api/v2'
export KDF_TON_API_KEY='...' # optional; keep outside Git and shell history where possible
```

## Start KDF and choose a wallet mode

```sh
cd /media/decker/data2tb/kdf-telegram/integration-runs/ton-gram
./start-kdf.sh hd
# or
./start-kdf.sh iguana
```

`start-kdf.sh` runs in the foreground. It creates a mode-specific database and listens
on port `17783` by default. Override `KDF_PORT`, `KDF_RUNTIME`, or
`KDF_TON_SEED_FILE` when required. The HD seed file must contain a normal KDF BIP39
mnemonic; do not place the phrase in this document, a command line, logs, or Git.

The two modes intentionally use different key sources:

| Mode | TON key source | Address policy |
| --- | --- | --- |
| `iguana` | KDF's existing 32-byte Iguana private-key material, interpreted as an Ed25519 seed | One deterministic TON wallet address |
| `hd` | KDF BIP39 seed, SLIP-10 path `m/44'/607'/0'`, then the TON W5R1 wallet key | One deterministic TON wallet address |

HD account/address creation and scanning are intentionally unavailable for GRAM. A
request for another HD address returns an error instead of deriving or mutating state.

## Common RPC setup

In a second terminal, set values for the KDF instance you started. `rpc` accepts a JSON
request on standard input and prints the reply.

```sh
export KDF_RUNTIME=/media/decker/data2tb/kdf-telegram/integration-runs/ton-gram
export KDF_PORT=17783
export KDF_RPC="http://127.0.0.1:$KDF_PORT"
export KDF_USERPASS="$(jq -r '.rpc_password' "$KDF_RUNTIME/config/hd.json")"

rpc() {
  curl --fail-with-body --silent --show-error \
    -H 'Content-Type: application/json' \
    --data-binary @- "$KDF_RPC"
}
```

For Iguana mode, use `config/iguana.json` in the `KDF_USERPASS` command. The following
examples use a public destination placeholder; replace it with a checked mainnet GRAM
address before sending value:

```sh
export DESTINATION='UQ...'
```

Build the provider-node array once for the activation calls:

```sh
export KDF_TON_RPC_NODE="${KDF_TON_RPC_NODE:-https://toncenter.com/api/v2}"
NODES="$(jq -cn --arg url "$KDF_TON_RPC_NODE" --arg key "${KDF_TON_API_KEY:-}" \
  'if $key == "" then [{url: $url}] else [{url: $url, api_key: $key}] end')"
```

## Activate GRAM

Legacy (v1) activation returns when the coin is ready:

```sh
rpc <<JSON
{
  "userpass": "$KDF_USERPASS",
  "method": "enable",
  "coin": "GRAM",
  "nodes": $NODES,
  "tx_history": true
}
JSON
```

The v2 endpoint starts an activation task. Save the returned `result.task_id`, then
poll its status until it is `Ok` or an error:

```sh
rpc <<JSON
{
  "mmrpc": "2.0",
  "userpass": "$KDF_USERPASS",
  "method": "enable_ton",
  "params": {
    "ticker": "GRAM",
    "activation_params": { "nodes": $NODES, "tx_history": true }
  }
}
JSON

export TASK_ID=1 # replace with result.task_id from the preceding reply
rpc <<JSON
{
  "mmrpc": "2.0",
  "userpass": "$KDF_USERPASS",
  "method": "task::enable_ton::status",
  "params": { "task_id": $TASK_ID, "forget_if_finished": false }
}
JSON
```

The legacy and v2 activation forms enable the same local GRAM coin. Do not activate it
twice in one KDF process.

## Address, balance, and history

Validate an address or convert its friendly representation to raw `workchain:hash`:

```sh
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"validateaddress","coin":"GRAM","address":"$DESTINATION"}
JSON

rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"convertaddress","coin":"GRAM","from":"$DESTINATION","to":"Raw"}
JSON
```

The current balance RPC is the legacy route, and it works after either activation form:

```sh
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"my_balance","coin":"GRAM"}
JSON
```

Read local transaction history through either envelope. History is synchronized after
activation when `tx_history` is true and is stored in KDF's mode-specific database.

```sh
# Legacy v1
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"my_tx_history","coin":"GRAM","limit":50}
JSON

# v2, HD wallet (the only supported HD account is 0)
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"my_tx_history",
  "params":{"coin":"GRAM","target":{"type":"account_id","account_id":0},"limit":50}
}
JSON

# v2, Iguana wallet
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"my_tx_history",
  "params":{"coin":"GRAM","target":{"type":"iguana"},"limit":50}
}
JSON
```

## Create and broadcast a transfer

First generate an unsigned signed-message BOC. `broadcast: false` makes this safe for
inspection; it does not send funds. Amounts accept up to nine GRAM decimal places.

```sh
rpc <<JSON
{
  "userpass":"$KDF_USERPASS",
  "method":"withdraw",
  "coin":"GRAM",
  "to":"$DESTINATION",
  "amount":"0.001",
  "broadcast":false,
  "expiration_seconds":60
}
JSON
```

Copy `tx_hex` from the result only after checking the destination and amount. Broadcast
the exact BOC once using the legacy raw-transaction RPC:

```sh
export TX_HEX='...' # result.tx_hex from the preceding response
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"send_raw_transaction","coin":"GRAM","tx_hex":"$TX_HEX"}
JSON
```

`send_raw_transaction` returns the external-message identifier reported by the
provider. It is not final proof of execution and is not necessarily the account
transaction hash. Check `my_tx_history` and the recipient balance after inclusion.
Do not regenerate or rebroadcast after an ambiguous network error until history has
been reconciled, because a newly built TON message may create another payment.

The common v2 withdrawal task is also available and broadcasts as part of the task:

```sh
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"task::withdraw::init",
  "params":{"coin":"GRAM","to":"$DESTINATION","amount":"0.001","from":null}
}
JSON

export WITHDRAW_TASK_ID=1 # replace with result.task_id
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"task::withdraw::status",
  "params":{"task_id":$WITHDRAW_TASK_ID}
}
JSON
```

## Stream balances and transaction-history events

Open the event stream before enabling a producer:

```sh
curl --no-buffer --silent --show-error "$KDF_RPC/event-stream?id=1"
```

In another terminal, enable balance and history streamers. The method names include
the `stream::` prefix.

```sh
rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::balance::enable","params":{"coin":"GRAM","client_id":1}}
JSON

rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::tx_history::enable","params":{"coin":"GRAM","client_id":1}}
JSON
```

Disable them by their returned streamer ID. The known IDs are `BALANCE:GRAM` and
`TX_HISTORY:GRAM`:

```sh
rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::disable","params":{"client_id":1,"streamer_id":"BALANCE:GRAM"}}
JSON

rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::disable","params":{"client_id":1,"streamer_id":"TX_HISTORY:GRAM"}}
JSON
```

## Reproducible runtime checks

`test-rpc.sh` activates GRAM, checks address handling, balance, v1/v2 history, and
both stream subscriptions. It defaults to a non-spending unsigned-transfer check.

```sh
cd /media/decker/data2tb/kdf-telegram/integration-runs/ton-gram
./test-rpc.sh hd '' legacy
./test-rpc.sh hd '' v2
./test-rpc.sh iguana '' legacy
./test-rpc.sh iguana '' v2
```

The following opt-in command sends `0.001` GRAM plus network fees. Use a funded,
disposable wallet and a destination you control:

```sh
./test-rpc.sh hd --send legacy
```

Stop foreground KDF with `Ctrl-C`. If it was started in the background by a local
automation, send `SIGINT` only to that recorded process ID:

```sh
kill -INT "$(cat "$KDF_RUNTIME/hd.pid")"
```

## Useful failures

| Symptom | Meaning and action |
| --- | --- |
| TON Center returns 429 or requests appear slow | No API key is in use; KDF caps anonymous requests at one per second. Set `KDF_TON_API_KEY` or wait and retry read operations. |
| `GRAM` is already enabled | Reuse the enabled coin, or restart KDF for another activation test. |
| New HD address/account/scan request fails | Expected current GRAM policy: only HD account 0 and its single address are supported. |
| Broadcast reply is ambiguous | Query history and provider state before attempting any new transfer. |
| Balance is zero after an activation | Check the selected mode, the enabled wallet address, mainnet node URL, and that the account is funded in GRAM rather than another TON asset. |
