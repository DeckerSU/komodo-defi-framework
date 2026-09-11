# GRAM (TON) with a KDF binary

This document is a self-contained terminal workflow for the KDF binary on the
`feat/ton-gram-integration` branch. It needs only the binary, `curl`, `jq`, a private
KDF configuration, and the GRAM-enabled **coins repository**. It does not depend on
the repository's test runtime, its scripts, or any local control seed.

GRAM is a mainnet, wallet-only TON coin with nine decimal places. KDF uses a TON W5R1
wallet in workchain 0, subwallet 0.

## 1. Get the required coins data

Use the `coins` repository branch `feat/ton-gram-integration`, at revision
`91f539e92351c22028ad5a7e3998a20497a84c17` or a descendant that retains the GRAM
entry. The required layout is:

```text
$COINS_ROOT/coins
$COINS_ROOT/seed-nodes.json
$COINS_ROOT/ton/GRAM
```

`$COINS_ROOT/coins` must contain the `GRAM` entry whose protocol is `TON`; the
`ton/GRAM` file supplies the default Toncenter v2 provider configuration. Point
`MM_COINS_PATH` at the first file, not the directory:

```sh
export COINS_ROOT=/absolute/path/to/coins
test -f "$COINS_ROOT/coins"
test -f "$COINS_ROOT/seed-nodes.json"
test -f "$COINS_ROOT/ton/GRAM"
```

The `coins` JSON array must contain this exact native-coin entry:

```json
{
  "coin": "GRAM",
  "name": "gram",
  "fname": "GRAM (TON)",
  "mm2": 1,
  "wallet_only": true,
  "decimals": 9,
  "avg_blocktime": 5,
  "required_confirmations": 1,
  "protocol": {
    "type": "TON",
    "protocol_data": {
      "network": "Mainnet",
      "wallet_version": "V5R1",
      "workchain": 0,
      "subwallet_number": 0
    }
  }
}
```

The adjacent `$COINS_ROOT/ton/GRAM` provider file can contain the public default:

```json
{
  "rpc_nodes": [
    { "url": "https://toncenter.com/api/v2" }
  ]
}
```

## 2. Create a private KDF configuration

Make an empty working directory and create `MM2.json` with mode `0600`. The following
commands request the KDF BIP39 phrase and RPC password interactively, so neither enters
shell history. The RPC password must include at least one special character, such as
`!`.

```sh
export KDF_HOME="$HOME/kdf-gram"
mkdir -p "$KDF_HOME/db"
chmod 700 "$KDF_HOME" "$KDF_HOME/db"

read -r -s -p 'KDF BIP39 phrase: ' KDF_PASSPHRASE; printf '\n'
read -r -s -p 'KDF RPC password: ' KDF_RPC_PASSWORD; printf '\n'
SEED_NODES="$(jq -ce '[.[] | select(.netid == 6133) | .host | select(type == "string" and length > 0)] | unique | select(length > 0)' \
  "$COINS_ROOT/seed-nodes.json")"
jq -n \
  --arg passphrase "$KDF_PASSPHRASE" \
  --arg rpc_password "$KDF_RPC_PASSWORD" \
  --arg dbdir "$KDF_HOME/db" \
  --argjson seednodes "$SEED_NODES" \
  '{
    gui: "nogui",
    netid: 6133,
    rpc_password: $rpc_password,
    passphrase: $passphrase,
    enable_hd: true,
    dbdir: $dbdir,
    rpcport: 17783,
    rpcip: "127.0.0.1",
    myipaddr: "127.0.0.1",
    disable_p2p: false,
    i_am_seed: false,
    is_bootstrap_node: false,
    seednodes: $seednodes,
    event_streaming_configuration: {
      access_control_allow_origin: "http://127.0.0.1"
    }
  }' >"$KDF_HOME/MM2.json"
unset KDF_PASSPHRASE KDF_RPC_PASSWORD
chmod 600 "$KDF_HOME/MM2.json"
```

This configuration starts **HD mode**. Its KDF BIP39 seed is derived with SLIP-10 path
`m/44'/607'/0'`, then used for the TON wallet. It does not require, or enable, a native
TON mnemonic mode.

The configuration starts a normal KDF light node: `disable_p2p`, `i_am_seed`, and
`is_bootstrap_node` are all explicitly `false`. `SEED_NODES` selects every host for
`netid: 6133` from the `coins` repository's `seed-nodes.json`; it is required because
KDF rejects a non-bootstrap node without configured seed nodes before its RPC server
starts.

For **Iguana mode**, use the same configuration structure but set `enable_hd` to
`false`, choose a separate `dbdir` and `rpcport` (for example `17784`), and start a
separate process. TON then interprets KDF's existing 32-byte Iguana private-key
material as an Ed25519 seed. The resulting address is deterministic, but differs from
the HD address.

GRAM currently supports exactly one address in either mode. HD account/address creation
and address scanning intentionally return an error.

## 3. Start KDF

Set `KDF_BIN` to the already built binary and run it with the config and coins data:

```sh
export KDF_BIN=/absolute/path/to/kdf
export MM_CONF_PATH="$KDF_HOME/MM2.json"
export MM_COINS_PATH="$COINS_ROOT/coins"
export MM_LOG="$KDF_HOME/kdf.log"
"$KDF_BIN"
```

KDF stays in the foreground. Keep it running and use a second terminal for RPC calls.
Stop it with `Ctrl-C` when finished.

## 4. Define the RPC helper and TON provider

In the second terminal, use the password from `MM2.json` and the configured port:

```sh
export KDF_RPC=http://127.0.0.1:17783
export KDF_USERPASS="$(jq -r '.rpc_password' "$KDF_HOME/MM2.json")"
export KDF_TON_RPC_NODE=https://toncenter.com/api/v2
export KDF_TON_API_KEY='' # optional

rpc() {
  curl --fail-with-body --silent --show-error \
    --connect-timeout 10 --max-time 90 \
    -H 'Content-Type: application/json' --data-binary @- "$KDF_RPC"
}

NODES="$(jq -cn --arg url "$KDF_TON_RPC_NODE" --arg api_key "$KDF_TON_API_KEY" \
  'if $api_key == "" then [{url: $url}] else [{url: $url, api_key: $api_key}] end')"
```

The public Toncenter endpoint works without an API key. KDF deliberately limits
anonymous provider requests to one per second. Set `KDF_TON_API_KEY` to use a provider
key; do not put it in `MM2.json`, the coins repository, or a command committed to Git.

## 5. Activate GRAM

Choose one activation form. Both activate the same local coin; do not invoke both in
the same KDF process.

### Legacy v1 activation

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

The successful response contains the wallet `address` and `balance`.

### v2 task activation

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
```

Save `result.task_id` from the reply and poll it until `result.status` is `Ok`:

```sh
export TASK_ID=REPLACE_WITH_RESULT_TASK_ID
rpc <<JSON
{
  "mmrpc": "2.0",
  "userpass": "$KDF_USERPASS",
  "method": "task::enable_ton::status",
  "params": { "task_id": $TASK_ID, "forget_if_finished": false }
}
JSON
```

## 6. Address, balance, and transaction history

Set a recipient or another address to inspect:

```sh
export ADDRESS='UQ...'
```

Validate a mainnet friendly or raw TON address and convert a friendly address to its
raw `workchain:hash` form:

```sh
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"validateaddress","coin":"GRAM","address":"$ADDRESS"}
JSON

rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"convertaddress","coin":"GRAM","from":"$ADDRESS","to_address_format":"Raw"}
JSON
```

The currently exposed balance endpoint is the v1 route and works after either v1 or v2
activation:

```sh
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"my_balance","coin":"GRAM"}
JSON
```

Read synchronized local history through either API. `tx_history: true` at activation
starts the KDF history worker.

```sh
# v1
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"my_tx_history","coin":"GRAM","limit":50}
JSON

# v2: HD mode; account 0 is the only supported account.
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"my_tx_history",
  "params":{"coin":"GRAM","limit":50,"target":{"type":"account_id","account_id":0}}
}
JSON

# v2: Iguana mode.
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"my_tx_history",
  "params":{"coin":"GRAM","limit":50,"target":{"type":"iguana"}}
}
JSON
```

## 7. Create, inspect, and broadcast a transfer

Set a validated destination. Amounts use decimal GRAM and accept at most nine decimal
places. This first request only creates a locally signed TON BOC; `broadcast: false`
does not send funds.

```sh
export DESTINATION='UQ...'
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

Review the returned `from`, `to`, `total_amount`, and `fee_details`. Save the returned
`tx_hex` exactly as received, then broadcast it **once**:

```sh
export TX_HEX='REPLACE_WITH_WITHDRAW_RESULT_tx_hex'
rpc <<JSON
{"userpass":"$KDF_USERPASS","method":"send_raw_transaction","coin":"GRAM","tx_hex":"$TX_HEX"}
JSON
```

`send_raw_transaction` returns an external-message ID in `tx_hash`. It means the
provider accepted the BOC; it is not necessarily the final account transaction hash.
Poll `my_tx_history` and check the recipient before creating another transfer after an
ambiguous error. Rebuilding and sending a new BOC can create a second payment.

The common v2 withdrawal task is also available. It sends as part of the task rather
than returning an offline BOC:

```sh
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"task::withdraw::init",
  "params":{"coin":"GRAM","to":"$DESTINATION","amount":"0.001","from":null}
}
JSON

export WITHDRAW_TASK_ID=REPLACE_WITH_RESULT_TASK_ID
rpc <<JSON
{
  "mmrpc":"2.0", "userpass":"$KDF_USERPASS", "method":"task::withdraw::status",
  "params":{"task_id":$WITHDRAW_TASK_ID}
}
JSON
```

## 8. Stream balance and history events

Open the server-sent-event endpoint in one terminal before enabling streamers:

```sh
curl --no-buffer --silent --show-error "$KDF_RPC/event-stream?id=1"
```

Enable the producers in another terminal. The `stream::` prefix is required:

```sh
rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::balance::enable","params":{"coin":"GRAM","client_id":1}}
JSON

rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::tx_history::enable","params":{"coin":"GRAM","client_id":1}}
JSON
```

Disable them when no longer needed:

```sh
rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::disable","params":{"client_id":1,"streamer_id":"BALANCE:GRAM"}}
JSON

rpc <<JSON
{"mmrpc":"2.0","userpass":"$KDF_USERPASS","method":"stream::disable","params":{"client_id":1,"streamer_id":"TX_HISTORY:GRAM"}}
JSON
```

## 9. Expected errors

| Reply or behavior | Action |
| --- | --- |
| Toncenter returns `429`, or reads are slow | KDF is using the anonymous one-request-per-second limit. Wait before retrying, or configure `KDF_TON_API_KEY`. |
| `GRAM` is already enabled | Reuse the coin in that KDF process. Restart KDF only when a fresh activation is intended. |
| New HD account/address/scan fails | Expected for GRAM: only HD account 0 and one wallet address are implemented. |
| Broadcast result is uncertain | Query KDF history and provider state before making any new transfer. |
| The balance is zero | Verify the enabled address, wallet mode, mainnet provider URL, and that the account holds native GRAM. |
