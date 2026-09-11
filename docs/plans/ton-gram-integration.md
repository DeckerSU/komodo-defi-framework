# TON / GRAM wallet integration plan

Status: implementation in progress; M0 and the TON primitive/key-derivation groundwork are complete.
Date: 2026-09-10.
Implementation branch: `feat/ton-gram-integration`, created from local `dev` at
`e686ef3500585f01c9f0e89c8c01bc036c42253c`.

This document is a handoff to the implementing LLM. Work only on this KDF branch.
Update this plan after each completed milestone and keep commits small and buildable.
The current request is for this plan only: coin implementation, coins database changes,
runtime packaging, and funded transaction tests below are subsequent work.

## 1. Requirements and scope

Implement TON native-currency wallet support under KDF ticker **GRAM**, protocol **TON**.
GRAM here means the native currency of the TON network, with 9 decimal places; it does
not mean a Jetton whose symbol happens to be GRAM. Do not select a token by symbol.

Required wallet operations: activation, address validation/conversion, balance,
fee estimation, transaction construction/signing, broadcast, confirmation tracking,
persistent transaction history, and disabling/shutdown. Implement both legacy KDF RPC
and `mmrpc: "2.0"` routes where applicable, task-based activation/withdrawal, and KDF
balance/transaction-history streaming.

The user's latest derivation requirements supersede the initial same-address request:

| KDF mode | Key source | Address expectation |
| --- | --- | --- |
| Iguana (`enable_hd: false`) | The existing 32-byte Iguana private key, interpreted as an Ed25519 signing seed | Deterministic W5R1 address; allowed to differ from the reference address |
| HD (`enable_hd: true`) | The primary KDF BIP39 phrase, derived through SLIP-10 at `m/44'/607'/0'` | Deterministic W5R1 address; it is intentionally distinct from a native TON mnemonic address |

Native TON reference fixture: workspace `seed.txt`, outside the KDF repository. It is
not a KDF HD fixture under the BIP39-only policy. Its mainnet address is:

```text
UQBYGTsWwxh00p3Fq_EdwzQ2uRzuptfxP5crEOsfRT6zDOS4
```

The user reports funding this address with 1.777 GRAM. Treat this as a reported initial
funding amount, not a balance verified during planning or an immutable test assertion.
The Iguana address has no reported funding.

Only one TON address is available in HD mode. New-address, new-account and scanning
operations must not create additional addresses or advance hidden counters. Return a
typed unsupported-operation error. Implement and test the pure W5 subwallet-address
calculation for future use, but do not expose additional addresses through RPC yet.

Wallet-only integration is the initial scope. Enforce `wallet_only` in configuration
and code. Atomic swaps, HTLC contracts, Jettons, NFTs, STON.fi, GasFree, hardware wallets,
and WalletConnect are separate future features. Required trait methods must return
explicit unsupported errors rather than panic or pretend that swaps work.

## 2. Repository findings

### 2.1 What KDF is

KDF is a Rust workspace implementing a noncustodial wallet and a peer-to-peer atomic-swap
DEX. It exposes JSON RPC, maintains orderbooks and swap state over libp2p, and delegates
blockchain operations to protocol implementations. A native binary is named `kdf`;
the historical `mm2` names remain throughout its crates and configuration.

Read these local instructions before implementation:

- Root `AGENTS.md` and workspace `../AGENTS.md`.
- `mm2src/coins/AGENTS.md`, `mm2src/coins_activation/AGENTS.md`.
- `mm2src/crypto/AGENTS.md`, `mm2src/mm2_main/AGENTS.md`.
- Instructions in any additional module being edited, including `mm2_bin_lib` or `common`.

Some module instructions reference a root `CLAUDE.md`; no such file was found in this
KDF checkout. Follow the available AGENTS instructions. The TRON sections of AGENTS
are partly stale: they describe token rejection, whereas current code supports TRC20.
Use the implementation and commit history to resolve these discrepancies.

Important integration boundaries:

| Area | Existing source |
| --- | --- |
| Protocol, coin, transaction and private-key enums; coin traits | `mm2src/coins/lp_coins.rs` |
| Fee response variants | `mm2src/coins/tx_fee_details.rs` |
| Crypto initialization and key sources | `mm2src/crypto/src/{crypto_ctx,global_hd_ctx,privkey}.rs` |
| Wallet import, encrypted storage and startup | `mm2src/mm2_main/src/lp_wallet.rs`, `lp_wallet/` |
| Activation lifecycle and cancellation | `mm2src/coins_activation/src/context.rs`, `standalone_coin/`, `platform_coin_with_tokens.rs` |
| Legacy and v2 routing | `mm2src/mm2_main/src/rpc/dispatcher/{dispatcher,dispatcher_legacy}.rs` |
| Task withdrawals and HD RPC dispatch | `mm2src/coins/rpc_command/` |
| History interface, storage and RPC | `mm2src/coins/my_tx_history_v2.rs`, `tx_history_storage/` |
| Stream activation | `mm2src/mm2_main/src/rpc/streaming_activations/{balance,tx_history}.rs` |
| Reusable history event pattern | `mm2src/coins/utxo/tx_history_events.rs` |
| Integration tests and helpers | `mm2src/mm2_main/tests/mm2_tests/`, `mm2src/mm2_test_helpers/` |

Use a separate `TonCoin`. TRON can reuse `EthCoin` because its address/key primitives
overlap with EVM. TON uses contract-state-derived addresses, Ed25519, cells/BOC and a
message-based execution model. Extending `ChainSpec` inside `EthCoin` for TON would
couple unrelated protocols.

### 2.2 How TRX was actually added, chronologically

The entries below were verified with `git show`, affected paths, and current callers,
not just inferred from commit titles. These five commits are ancestors of the current
KDF base. The feature PRs are squashed commits: their internal authoring order cannot
be recovered as separate commits from the merged history.

| Date | Commit / PR | Logical block delivered | What was still missing |
| --- | --- | --- | --- |
| 2025-05-10 | `ee6bdd7b8`, #2425 | Initial `ChainSpec` separation inside `EthCoin`; TRON network enum; address parser, Base58Check/hex serialization, validation and tests; protocol/activation plumbing | TRON client and fee types were placeholders. This was not a complete activation or payment implementation |
| 2026-01-12 | `8404fa530`, #2467 | Working activation through the ETH pipeline; `TronApiClient` HTTP access with retries/failover; `ChainRpcClient`/`ChainRpcOps`; chain-tagged display addresses; native balances and current block; HD account/path support and gap scanning; optional swap contract for wallet-only use; immediate/task RPC integration tests | No complete signing/withdrawal pipeline; tokens still rejected in this stage |
| 2026-03-04 | `a12695e13`, #2712 | TRC20 configuration, contract address/decimals/balance queries, platform-plus-token and token-only activation; HD balance/scanning integration | Native/TRC20 transfer construction and broadcast arrived next |
| 2026-03-06 | `7c9893b66`, #2714 | Minimal protobuf types; TAPOS unsigned builders; SHA256/secp256k1 signing; bandwidth/energy fees; TRX/TRC20 withdrawal flows, max-send calculation; broadcast dispatch to `/wallet/broadcasthex`; `TxFeeDetails::Tron`; request expiration; Iguana/HD withdrawal tests | A destination account-creation fee edge case remained; no dedicated TON-like history/streaming solution to copy |
| 2026-03-27 | `1f984dd11`, #2724 | Destination activation check; account-creation fees in estimates/max-send and GUI fee breakdown; regression tests for activated/unactivated recipients | TRON-specific history and swaps remain outside this series |

The withdrawal commit explicitly identifies itself as the fourth PR in the series
#2425 → #2467 → #2712 → #2714. Thus address support came first, working activation and
HD second, token read operations third, and build/sign/fee/withdraw/broadcast together
in the fourth merged feature. Do not describe signing and broadcast as separate
historical merged commits when the available history groups them together.

Other branches contain experiments in May–June 2025 and later GasFree work. They are
not additional completed stages on this base. In particular, `origin/feat/tron-gasfree`
contains provider configuration (`0fa59f371`, April 7), CREATE2 addresses (`194130c71`,
April 16), HTTP/schema (`66e238ea4`, April 21), account service (`abe0cdbb4`, April 23),
withdraw schemas (`3fce5a315`, April 23), TIP-712 signing (`18553a141`, April 23), and
sign-only gasless withdrawal (`7f8d735d3`, May 4). Do not assume these changes exist on
`feat/ton-gram-integration` or cherry-pick them as a prerequisite.

### 2.3 TRX: Iguana versus HD

TRX uses the existing key-policy machinery rather than its own mnemonic algorithm:

- Iguana: `PrivKeyBuildPolicy::IguanaPrivKey` supplies the existing secp256k1 key.
  The TRON address is derived from the Ethereum-style public-key address bytes,
  prefixed with `0x41`, and encoded as Base58Check.
- HD: `GlobalHDAccountCtx` validates BIP39, creates BIP32 key material, and derives
  secp256k1 children. `coins/coins` specifies `m/44'/195'`; account/change/index extend
  that path, for example `m/44'/195'/0'/0/0`.
- `EthHDAddress` wraps `ChainTaggedAddress`; formatting is selected by chain family.
  Public-key extraction and account management reuse the ETH HD abstractions.
- HD scanning checks on-chain account existence, not just nonzero balance; TRC20
  support subsequently also checks configured token balances. Tests cover address
  indices, another account, gap limits, and previously used zero-balance addresses.
- Both modes use v2 activation machinery. Legacy ETH `enable` explicitly rejects
  TRON: **Iguana is a key mode, not a synonym for legacy/v1 RPC**.
- Withdrawal dispatch selects TRON key/build/sign/fee logic while retaining common
  withdrawal tasks. `tron_tests.rs` covers Iguana broadcast and HD withdrawals.

Current TRX is not a working history template: `EthCoin::process_history_loop` still
uses EVM history paths, v2 history routing has no `EthCoinVariant`, and transaction
history streaming does not dispatch to TRON. Balance streaming has ETH machinery
that can serve as an architectural reference. Implement TON history explicitly.

### 2.4 What to reuse from `../ton-send`

Inspected revision: `e312fe2d6a6622288698ac344d79f20d26710ab5`, branch `stonfi-swap`.
The untracked local `CLAUDE.md` was not modified.

| Source | Reusable behavior | Adaptation required |
| --- | --- | --- |
| `src/wallet.rs` | 32-byte seed → Ed25519 key pair; `TonWallet`; internal message fields | Feed KDF's Iguana bytes or the BIP39/SLIP-10-derived seed; fix wallet parameters explicitly |
| `src/main.rs` | Read state, build/sign external message, include StateInit for first send, BOC broadcast | Move logic into RPC/task-compatible operations; no CLI confirmation inside KDF |
| `src/util.rs` | Integer amount parsing and friendly-address flags | Preserve strict checksum/tag/network validation; add raw/base64 variants and checked bounds |
| `src/toncenter.rs` | `getWalletInformation`, `sendBocReturnHash`, endpoint selection, API-key header, retry outline | Replace blocking `ureq` and `thread::sleep` with KDF async transport/timers; typed responses and bounded retries |
| Existing tests | Internal-message and signed external-message round trips | Add W5 vectors, cryptographic signature verification, malformed inputs and KDF integration tests |

Do not reuse its fixed 0.01 TON reserve as fee estimation. Do not treat every
non-active state as deployable: frozen/unknown accounts need explicit errors.
Do not default an active account's missing/malformed `seqno` to zero. A successful
broadcast returns a message reference, not proof that the recipient was credited.

KDF uses `tonlib-core = 0.26.11` plus `nacl = 0.5.3`. It provides
`Mnemonic::from_str`, `Mnemonic::to_key_pair`, `TonWallet::new_with_params`, W5R1,
cells and BOC support without a network client. The initially evaluated `ton = 0.4.0`
crate pulled an unconditional reqwest/Tokio/mio graph and failed the WASM build; it is
therefore unsuitable as KDF's protocol primitive dependency. `tonlib-core`'s public
mnemonic errors can include an offending word, so KDF boundaries must sanitize them.
Its `TonWallet` owns a `Vec<u8>` secret key that is not zeroized on drop. The initial
primitive wipes the short-lived NaCl keypair copy; M2/M5 must keep durable private
material in zeroizing KDF-owned storage and create signing-library temporaries only
when required. Avoid an unrelated workspace-wide crypto upgrade.

### 2.5 Fixture preflight and a critical HD startup constraint

`seed.txt` is a numbered list of 24 words, not a space-separated mnemonic. After the
user corrected the file, local verification parsed sequential numbering without
printing words and successfully validated the passwordless TON mnemonic through
`tonlib-core` 0.26.11. `Mnemonic::from_str` → `to_key_pair` →
`TonWallet::new_with_params(V5R1, key, 0, 0x7fffff11)` produced exactly:

```text
UQBYGTsWwxh00p3Fq_EdwzQ2uRzuptfxP5crEOsfRT6zDOS4
```

The corrected phrase **does not pass the BIP39 checksum**. The startup constraint
below is therefore confirmed for this fixture, not merely a possible edge case.
Do not silently correct, guess, reorder, or commit any words. Revalidate the address
if the local fixture changes again.

KDF currently executes this path before any coin activation:

```text
initialize_wallet_passphrase
  → initialize_crypto_context(enable_hd = true)
  → CryptoCtx::init_with_global_hd_account
  → GlobalHDAccountCtx::new
  → bip39_seed_from_mnemonic (strict BIP39 validation)
```

Native TON mnemonics use the English word list but a different validation and key
derivation scheme. A valid TON mnemonic need not pass BIP39 validation; BIP39-derived
SLIP-10 keys do not reproduce the default TON mnemonic wallet. Also, the existing HD
context retains derived material rather than the original phrase. Merely adding a
TON coin builder is therefore insufficient. [TON mnemonic implementation](https://github.com/ton-org/ton-crypto/blob/master/src/mnemonic/mnemonic.ts).

## 3. Design decisions

### 3.1 Wallet identity and key modes

Use W5R1, workchain 0, mainnet wallet ID `0x7fffff11` (2147483409), subwallet counter 0
for the reference wallet. Explicitly use the testnet wallet ID `0x7ffffffd` for testnet.
Changing only an address's testnet display flag is insufficient: W5 uses the network
in its wallet ID. Fix these values in tests and avoid silently relying on library
defaults. [Official W5 wallet derivation](https://docs.ton.org/contracts/standard/wallets/interact).

Iguana must reuse `IguanaPrivKey` bytes exactly. Existing KDF passphrase processing can
accept WIF/hex or hash and clamp a passphrase; do not substitute plain SHA256, random
bytes, or TON mnemonic derivation for that established behavior. Feed the resulting
32 bytes to Ed25519, then construct W5R1. It must not use a native TON mnemonic
or the BIP39 HD derivation path.

For HD, use KDF's existing strict BIP39 startup and derive the Ed25519 signing
seed through the TEP-3 multichain SLIP-10 path `m/44'/607'/0'`. This uses the same
primary phrase as every other KDF HD coin, with no global `mnemonic_type`, no second
key-policy variant, and no native-TON fallback. The selected path is a constant, not
an RPC input; keep account/change/address selectors disabled until a compatible
multi-address design is specified. Model public metadata as `bip39_slip10`, W5R1,
network/workchain/subwallet information.

The corrected local `seed.txt` remains a useful native TON reference, but it is not
BIP39-valid and therefore cannot initialize an HD KDF wallet under this policy. Its
funded native address must not be presented as a KDF HD address or used by automated
KDF tests. The private control fixture `../seed-bip39-control.txt` contains a freshly
generated BIP39 phrase; its public W5R1 mainnet address is
`UQCLuOL1GAZuZbbhocUlGI3gxasW9HNK8zZpN7L-noXSF9l0`.
It was funded with `1.776019966 GRAM` on 2026-09-11; treat that as a live-test
starting condition, not a fixed balance assertion.

Startup contract:

- `enable_hd: true` always initializes `GlobalHDAccountCtx` from the primary BIP39
  phrase. Existing coin derivation, encrypted wallet storage and database identity
  remain unchanged.
- GRAM activation receives `PrivKeyBuildPolicy::GlobalHDAccount` and derives only
  `m/44'/607'/0'` through the existing Ed25519 SLIP-10 implementation.
- Iguana activation reuses its existing 32-byte private key as an Ed25519 signing
  seed. It remains independent of HD derivation.
- Do not accept, persist, validate, or derive a native TON mnemonic in the KDF global
  wallet context. A future explicit TON-key import can be designed as a separate
  encrypted coin secret without changing this contract.

This uses no new startup configuration and does not weaken the existing BIP39
validator globally.

### 3.2 One address now; deterministic additional addresses later

Represent the initial TON wallet as a single wallet identity, not an invented xpub
or a scan-capable BIP32 account. Expose account 0/address 0 in an explicitly TON-aware
balance response when HD clients need account structure.

Add `AdditionalAddressesNotSupported` (or an equivalently precise common typed error)
to immediate/task new-address and account-creation responses. Reject nonzero account,
change/internal-chain, address-index or alternative-path selectors on activation,
withdrawal and account queries. The default selector resolves to the same sole wallet.
Reject scanning requests that could imply discovering more wallets. Repeated rejected
requests must leave persistent state unchanged.

Future addresses are possible without a new seed: W5's wallet ID encodes a subwallet
counter, so different counters produce different StateInit/address values with the
same public key. Add a small pure helper accepting a bounded counter (0..32767),
network and workchain, with vectors for 0, 1 and the bound. Production activation
must still allow only counter 0. These are separate contracts sharing a signing key,
not independently derived child keys. [Official W5 wallet ID scheme](https://docs.ton.org/contracts/standard/wallets/v5).

TON also has a mnemonic-to-HD-seed/SLIP-10 mechanism, but adopting it would create a
different derivation contract and would not preserve the default mnemonic wallet.
Defer public multi-address support until its recovery metadata, discovery and UX are
specified. [TON crypto HD primitives](https://github.com/ton-org/ton-crypto).

### 3.3 RPC and serialization contracts

Use proposed `enable_ton` for immediate activation in both KDF envelopes and
`task::enable_ton::{init,status,cancel,user_action}` for the existing task lifecycle.
Prefer standalone activation traits for the first native-coin implementation; TON's
ability to host future Jettons does not require implementing token initialization now.

`withdraw` signs and returns a transaction without broadcasting. The same applies to
task withdrawals. `send_raw_transaction` performs the broadcast separately. Reject
unsupported `broadcast: true` behavior explicitly instead of ignoring it.

Use existing KDF `tx_hex` for **hex-encoded serialized BOC bytes**. Convert to base64
only at the Toncenter HTTP boundary. Define input-size/cell-depth/reference limits,
root count, external-in message type, wallet destination and network checks.
Never hash the BOC file bytes as though they were a TON cell hash.

Distinguish three identifiers: signed message hash, normalized external message hash,
and eventual account transaction hash. Before inclusion, return a message identifier
with documented type; if common KDF fields force a provisional `tx_hash`, mark its
kind explicitly in TON metadata. Confirmed history uses the actual account transaction
hash. Persist the association rather than promising these hashes are identical.
Normalization matters when locating externally submitted messages. [Official message
lookup and normalization](https://docs.ton.org/applications/ton-connect/how-to/message-lookup).

| Operation | Legacy RPC | KDF v2 RPC | Implementation note |
| --- | --- | --- | --- |
| Immediate activation | `enable_ton` (new) | `enable_ton` (new) | Shared typed activation service; adapt envelope only |
| Task activation | Not needed | `task::enable_ton::*` (new) | Init/status/cancel and conventional user-action handling |
| Balance | `my_balance` | `my_balance` (add typed route if absent) | Existing generic legacy handler; inspect v2 dispatcher explicitly |
| Address validation/conversion | `validateaddress`, `convertaddress` | Same proposed names if typed routes are absent | Preserve existing legacy names; typed TON format params |
| Withdrawal | `withdraw` | `withdraw`, `task::withdraw::*` | Add `TonCoinVariant` to task dispatch |
| Broadcast | `send_raw_transaction` | Same name (add typed route if absent) | Envelope adapters share BOC validation/broadcast logic |
| History | `my_tx_history` | `my_tx_history` | Two existing handler families; explicitly add TON to each |
| HD account balance | No new legacy account API needed | `account_balance` | One TON account/address; typed TON metadata |
| New address/account/scan | Any exposed alias must reject | `get_new_address`, task new-address/new-account/scan | Typed unsupported error; no side effects |
| Streaming | No separate v1 stream protocol | `stream::balance::enable`, `stream::tx_history::enable`, `stream::disable` | Existing KDF event manager and SSE transport |
| Disable | `disable_coin` | Existing route or typed adapter | Cancel all TON tasks and subscriptions |

Verify exact existing names/parameter schemas in dispatchers before implementing
adapters. Do not assume a legacy handler is available under `mmrpc: "2.0"` just because
`withdraw` is. KDF API versions and Toncenter API v2/v3 are unrelated version systems.

## 4. Incremental implementation milestones

Each milestone includes its tests and should end in a small coherent commit or series
of commits. The sequence follows TRX's progression from primitives to activation to
payments, then adds history and streaming explicitly.

### M0 — Reproducible fixtures and dependency decision

- [x] Recheck branch/base and all applicable instructions; preserve unrelated edits.
- [x] Validate the corrected numbered seed file locally. Accept plain words or exactly
  ordered numbered lines; reject mixed numbering and extra data. Do not print words.
- [x] Offline derive the native TON W5R1 reference with `tonlib-core`; it confirms the
  corrected local fixture but is not KDF HD startup validation. The BIP39/SLIP-10 W5R1
  vector is separately covered in the KDF primitive tests.
- [x] Derive Iguana independently using existing KDF key construction and record its
  public expected address. The disposable `kdf-ton-iguana-vector` passphrase produces
  `UQBZfhh5F-CFw-1L978b7jrJ0c3FUlssE8jk2ueScxRHleke`; it is intentionally distinct
  from the HD reference address.
- [x] Select/lock the minimal usable dependency configuration: `tonlib-core 0.26.11`
  and `nacl 0.5.3`; check native and WASM compatibility. The initial `ton 0.4.0`
  candidate was rejected because its unconditional native transport dependencies fail
  WASM. Avoid local path dependencies on `../ton-send` in shipped KDF.
- [x] Add public disposable mnemonic/key/address vectors. The funded seed must never
  become a unit test fixture, source literal, test name or CI secret requirement.

Acceptance: two explicit key-source vectors, dependency decision, and an HD reference
comparison that passes only with a valid user fixture. If the fixture remains invalid,
public-fixture development can continue but funded acceptance remains blocked.

### M1 — Protocol, address and wallet primitives

- [x] Add `mm2src/coins/ton/` with small `mod.rs`, address/wallet/error modules; split
  files only as needed. Reuse `tonlib-core` cell/BOC/signature code rather than
  reimplementing it.
- [x] Add `CoinProtocol::TON`, typed network/W5 parameters, and
  `MmCoinEnum::TonCoinVariant` so GRAM can enter KDF's common coin registry.
  A TON transaction representation and `TxFeeDetails::Ton` belong with the withdrawal
  implementation, where the final fee response shape is known.
- [x] Encode nano amounts with bounded checked integer arithmetic; no floats, overflow,
  negative/zero send amounts, or fractional precision beyond 9 decimals.
- [ ] Preserve friendly address flags separately from the raw workchain/account hash.
  Support raw and valid standard/URL-safe friendly forms; validate CRC, tags and length.
  Reject testnet-only destination tags on mainnet; raw addresses use an explicit network
  context and a documented bounce default.
- [x] Implement Iguana raw-seed and BIP39/SLIP-10 HD W5 construction and the private
  future-subwallet helper. Make public keys distinct from contract addresses.
- [x] Implement required coin/swap trait errors without `todo!`, `unimplemented!` or
  hidden panics. `TonCoin` owns the validated TON wallet identity and implements
  `MarketCoinOps`, `WatcherOps`, and `MmCoin`. It exposes the W5 address and native
  balance, advertises itself as wallet-only regardless of request data, and makes all
  unsupported raw-BOC, confirmation, history, HTLC, key-export, arbitrary-message,
  withdraw, and trading operations return explicit errors. The legacy infallible
  HTLC-key hooks return compatibility zero values but are unreachable because
  `MmCoin::wallet_only` is enforced. Actual withdrawal, history, and confirmation
  tracking remain later milestones.

Tests: reference vectors, CRC corruption, tags, network/workchain changes, wallet-ID
changes, invalid mnemonic redaction, max integer/decimal bounds, BOC round trips and
subwallet counter uniqueness/bounds.

### M2 — BIP39/SLIP-10 HD derivation and single-address behavior

- [x] Remove the global `mnemonic_type: "ton"`, native-TON key context, and TON-only
  key-policy variant. HD startup remains the existing BIP39 path for every coin.
- [x] Derive GRAM's Ed25519 signing seed using TEP-3 `m/44'/607'/0'`; add a public
  BIP39→SLIP-10→W5R1 vector without exposing a mnemonic.
- [x] Bind the existing KDF `PrivKeyBuildPolicy` to a non-serializable, zeroizing
  TON signing-seed type: Iguana copies its 32 existing private-key bytes exactly;
  HD derives the fixed TEP-3 path from `GlobalHDAccountCtx`; Trezor and WalletConnect
  return explicit unsupported errors. This is the activation key-source boundary;
  it does not yet persist wallet metadata or register a coin.
- [ ] Preserve named/encrypted wallet metadata and deterministic DB identity across restart.
- [ ] Wire account-balance and selector handling for the sole TON address.
- [ ] Add unsupported-new-address/account/scan handling at both direct and task boundaries.
- [ ] Update relevant AGENTS documentation for the TON HD path and single-address policy.

Tests: valid BIP39 wallets retain their existing addresses and derive the documented
GRAM vector; a native-TON-only mnemonic remains rejected by normal KDF HD startup;
encrypted reload preserves the same GRAM address; every additional-address request
fails without changing storage.

### M3 — Async network client and activation/balance

- [x] Implement the initial narrow async Toncenter v2 client using `mm2_net`. It validates
  a credential-free `/api/v2` endpoint, keeps API keys in zeroizing storage, uses
  `X-API-Key`, and applies a request deadline on native and WASM transports.
- [x] Add typed `getWalletInformation` parsing for balance, account state, wallet type
  and optional seqno. Never coerce an absent active-account seqno to zero.
- [x] Add a strict `runGetMethod(seqno)` fallback for an active wallet when
  `getWalletInformation` omits `seqno`; it validates a successful numeric stack result
  and never turns a missing value into zero. Add bounded `sendBocReturnHash` support
  for already-signed BOCs; its result is deliberately a provider message reference,
  not a confirmed account transaction. Masterchain-head, fee and history calls remain
  for their consuming milestones.
- [ ] Distinguish nonexist/uninitialized, active, frozen and unknown states. A typed
  transfer-state helper now permits StateInit only for uninitialized accounts, requires
  a real active-account seqno, and rejects frozen/unknown state. Active accounts still
  need a compatible W5 code/state check before activation and withdrawal.
- [ ] Add request deadlines, bounded retries/backoff, cancellation, endpoint failover
  and rate limits shared by background consumers. The initial bounded read-only pool now
  tries each configured endpoint once for transport/timeouts/429/5xx and never retries a
  broadcast automatically; backoff, cancellation, rate limits and network identity remain.
  Do not turn provider errors into zero balances. Verify selected endpoints/network using
  a supported network identity check.
- [ ] Implement immediate/task activation and balance; expose actual normalized address,
  network, wallet version, key mode and supported capabilities. The legacy v1 `enable`
  path now accepts exactly one explicit `nodes` or `rpc_nodes` array, validates the
  native GRAM configuration and KDF key policy, fetches wallet information before
  registration, and allows only deployable or correctly sequenced active accounts.
  `tx_history: true` is rejected until history exists. The task (v2) endpoint, a typed
  activation result, W5 code/state verification, cancellation integration, and a
  capability response remain.
- [ ] Add feature-gated network tests following `tron-network-tests`; ordinary tests
  use deterministic HTTP fixtures/mock servers without funded credentials.

Tests: both modes; duplicate enable; wrong protocol; no endpoints; failover; 429/5xx;
bad JSON/TON error envelopes; missing active seqno; frozen wallet; cancellation; balance
precision; used zero-balance account; stop/disable while requests are in flight.

### M4 — Coordinated `coins` repository entry

Inspected coins base: `7158b94dfaa11ca6117be2425f86ecda606ab881`, local `master`.
No GRAM, TON or SOL entry exists in its root `coins` file. TRX is wallet-only and its
HTTP endpoints currently live in `ethereum/TRX`; do not infer a `tron/` directory exists.

- [x] Create a matching `../coins` integration branch before modifying that separate
  repository: `feat/ton-gram-integration` at `512ac11e`. KDF's branch does not
  protect files in another Git repository.
- [x] Add the single native GRAM entry to `../coins/coins`; it has `wallet_only: true`,
  9 decimals, one confirmation, W5R1 Mainnet/workchain 0/subwallet 0, and no
  invented BIP44 path, swap contract, or EVM metadata. Schema:

```json
{
  "coin": "GRAM",
  "name": "gram",
  "fname": "GRAM (TON)",
  "mm2": 1,
  "wallet_only": true,
  "decimals": 9,
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

- [x] Finalize the entry against `CoinProtocol::TON`'s strict parameters and validate
  JSON/ticker uniqueness locally. KDF activation still needs `TonCoin` before it can
  load the artifact at runtime.
- [x] Add `ton/GRAM` provider metadata with the explicit TON Center v2 endpoint,
  `explorers/GRAM`, and native TON-directory support in `utils/generate_app_configs.py`.
  Provider credentials are not stored in the coins repository.
- [x] Ensure GRAM is classified as a native TON platform with HTTP nodes, never an
  Electrum/UTXO/EVM coin. The config generator reads `ton/GRAM` directly.
- [ ] Update README classification; add only legitimate existing branding if needed.
  Avoid unrelated icon/config regeneration or network scans during this change.
- [x] Validate JSON, ticker uniqueness, exact decimals/network/W5 settings, provider and
  explorer fixtures, plus generator Python syntax. KDF must load the actual updated
  coins artifact successfully once `TonCoin` activation exists.

The test runtime must copy **`../coins/coins`**, not the coins repository directory or
only a generated GUI configuration. Include separate provider metadata if launch
scripts read it for activation. Record both repository revisions in the test manifest.

### M5 — Build, sign and estimate withdrawals

- [ ] Implement internal native transfer and optional bounded UTF-8 comment payload;
  respect destination bounce semantics and reject unsupported payload/fee selectors.
- [ ] Fetch fresh sender state, balance and seqno. Include initial W5 StateInit only
  for a deployable account; verify it hashes to the sender address.
- [ ] Build/sign W5 external messages with explicit wallet ID, send mode and checked
  expiration. Reuse `WithdrawRequest.expiration_seconds` with documented bounds.
- [ ] Estimate the actual message through Toncenter `estimateFee`; pass the API's expected
  body/init code/data rather than confusing it with the complete broadcast BOC.
  Capture storage, compute, action/forwarding costs without double-counting, and expose
  which values are estimates. Model first-wallet deployment and recipient cases.
- [ ] Verify message/signature sizes and destination before returning `TransactionDetails`.
- [ ] Implement `max` using a bounded convergence algorithm with conservative reserve
  semantics tied to estimated fees. Detect nonconvergence and insufficient balance.
  Never implement max-send by blindly using a wallet-destroying send mode.
- [ ] Preserve common `withdraw` and task behavior; return signed BOC hex and TON metadata
  (seqno, expiration, message identifier and fee currency GRAM), without broadcasting.

Tests: signature verification with a public fixture; decode every message field; first
send vs deployed wallet; memo length/Unicode; expiration bounds/overflow; incorrect key;
unsupported selector; empty/zero/negative/overprecision amount; exact insufficient funds;
max convergence/nonconvergence; fee-provider failure. Sandbox/emulation tests should
exercise W5 compute and action failures before relying on funded tests.

Toncenter documents fee estimation separately from broadcasting. Use recorded schema
fixtures and verify the deployed provider's compatibility with W5. [Estimate fee API](https://docs.ton.org/api/v2/send/estimate-fee).

### M6 — Broadcast, replay handling and confirmations

- [ ] Add shared legacy/v2 `send_raw_transaction` implementation; bound and decode BOC,
  validate supported message shape and broadcast the identical signed bytes.
- [ ] Track in-flight messages by wallet/seqno/message identity with bounded state.
  Parallel withdrawal builds can target the same current seqno; document that they are
  alternatives, not a guarantee of multiple executable transactions. Reject conflicting
  pending sends and stale seqnos, or return a precise rebuild-required error.
- [ ] A timeout after sending means unknown outcome. Reconcile using the exact message
  reference and chain state; do not automatically sign a replacement transfer. Advancing
  seqno alone does not prove that this particular payment succeeded.
- [ ] Resolve normalized external-message hash to the included sender transaction and
  inspect compute/action result plus the actual outgoing message. Follow recipient
  execution/bounces before reporting delivered value. W5 send-mode error suppression
  can leave seqno advanced without a successful transfer.
- [ ] Define confirmation units using masterchain inclusion. Never subtract a shard
  seqno or account logical time from masterchain height. Preserve shard/block/LT metadata.
- [ ] Persist enough pending state to reconcile after restart; expire/cancel trackers
  without claiming that local cancellation reverses an already submitted payment.

Tests: successful send; malformed BOC; invalid signature; same-BOC retry; conflicting
same-seqno messages; rejection; timeout-before/after acceptance; restart reconciliation;
expired message; sender compute/action failure; recipient bounce; confirmation timeout.

### M7 — Transaction history, legacy and v2

- [ ] Add paginated account history using Toncenter v2 `getTransactions` and/or v3 indexed
  transactions with explicit provider capabilities. v3 can resolve messages/traces and
  masterchain associations; v2 pagination uses an account LT/hash cursor.
- [ ] Store actual account transaction hash, LT, account/network/wallet identity, time,
  incoming/outgoing messages, fees and status. Stable internal IDs must not collide when
  one transaction contains multiple transfers. Use checked storage conversions.
- [ ] Reuse `TxHistoryStorage`/`TxHistoryStorageBuilder` native and WASM implementations.
  Add a small TON cursor/pending-mapping table only if existing storage cannot hold the
  necessary metadata. Do not create an unrelated database framework.
- [ ] Namespace by network and wallet identity so Iguana, HD and future subwallets cannot
  contaminate each other's history. Avoid treating formatting variants as different wallets.
- [ ] Implement bounded initial backfill, incremental catch-up, overlap deduplication,
  persisted cursor, temporary errors, finality updates and restart recovery.
- [ ] Connect both `my_tx_history_v2_rpc` and the legacy history handler to the same
  TON source of truth. Do not leave legacy reads pointing to a separate stale JSON file.
- [ ] Expose synchronization state and correct balance effects for incoming, outgoing,
  self-transfer, fees, deployment, bounced and failed transactions. A self-transfer is
  not a zero-cost no-op; prevent double-counting in aggregate wallet balance changes.

Tests: multi-page history, equal timestamps, LT ordering, uint bounds, overlap/restart,
provider lag, pruned history, receive-only address, pending→confirmed association,
multiple out-messages, bounce and self-transfer accounting, legacy/v2 parity.
History must not be marked complete when a provider cannot supply older pages.
[Toncenter transaction schema](https://docs.ton.org/ecosystem/api/toncenter/v3/blockchain-data/get-transactions).

### M8 — Balance and transaction-history streaming

- [ ] Add TON variants to the existing streaming activators and use the coin's abortable
  spawner. The existing native SSE endpoint is `/event-stream`.
- [ ] Publish history changes after successful persistence; reuse the existing history
  event streamer if its events fit, or add a focused TON implementation. Do not create
  an independent poller per subscriber or unbounded queues.
- [ ] Support initial balance and later changes, transaction discovery/status updates,
  multiple clients, unsubscribe, disable, shutdown and catch-up through history RPC.
- [ ] Prefer the existing polling/history worker as the first event source. Provider
  streaming is optional and must not be necessary for KDF streaming to work.

Tests: attach client before subscription, streamer IDs, no duplicate worker, event
contents vs history, bounded slow-client behavior, unsubscribe, reconnect/catch-up,
disable and shutdown. Provider outages must not emit a false zero balance.

### M9 — Packaged runtime, curl tests and release checks

- [ ] Add versioned scripts under `scripts/ton/`: `prepare-runtime.sh`, `start-kdf.sh`,
  `test-rpc.sh`, and a small seed/config helper if shell alone would expose secrets.
- [ ] Build release KDF and create an ignored `integration-runs/ton-gram/` directory:

```text
integration-runs/ton-gram/
  kdf
  coins
  start-kdf.sh
  test-rpc.sh
  manifest.json                 # KDF/coins revisions, toolchain, hashes; no secrets
  config/                      # generated private MM2.json files
  db/iguana/                   # isolated DB and history
  db/hd/
  logs/                        # redacted
  results/                     # assertions and public tx/message references
```

- [ ] Copy the built binary and actual modified coins file; copy script/helper dependencies.
  Document prerequisites (`curl`, `jq`, config helper interpreter if used) and artifact
  checksums. Scripts must resolve paths relative to themselves and work outside repo CWD.
- [ ] `start-kdf.sh hd` defaults to the private BIP39 control seed file and the standard
  KDF HD startup config. `start-kdf.sh iguana` uses the canonical phrase through existing Iguana
  processing. Use `umask 077`, private config permissions, loopback RPC, generated RPC
  password and separate ports/DBs. Put JSON in files, not a secret-bearing argv.
  Do not enable shell tracing or copy `seed.txt` into the package.
- [ ] Configure native SSE using the existing event-stream configuration schema. Check
  startup readiness with bounded retries; trap exit/signals and shut down only the
  process started by the script. Never use global `killall kdf`.
- [ ] Implement assertions using curl and jq, not only demonstrations that print responses.
  Set connect/request/polling timeouts, propagate failures with nonzero exit status and
  redact credentials. Exit zero only when all selected tests pass; report skips distinctly.
- [ ] Provide a default read-only suite and an explicit `--send` suite. The requested
  final implementation validation includes sending; ordinary reruns must not spend
  funds silently. Use a fixed small amount, explicit destination and fee/spend cap.
  Max-send and insufficient-balance tests build/mock only; never drain the funded wallet.

Required runtime RPC sequence (execute for both modes, with funded sends where funds
exist; task routes and HD-specific cases apply as indicated):

1. Start, authenticate, query version/health, load GRAM config, attach SSE client.
2. Activate with immediate legacy and v2 APIs in separate clean runs; test v2 task
   activation/status/cancel. Assert wallet mode, network, decimals and address.
3. For HD, assert the BIP39/SLIP-10 address derived from the private control fixture.
   For Iguana, assert its independent offline vector. Query balance and compare with a bounded contemporaneous provider
   observation if enabled; do not require the original 1.777 after funds move.
4. Validate UQ/EQ/raw representations and conversion; reject checksum/network errors.
5. Query HD account balance, then request new address/account/scan through all supported
   routes. Assert typed errors and unchanged address/history/account counts.
6. Query history through both versions; assert pagination/schema/accounting parity.
7. Enable balance/history streams and verify subscriptions. Use bounded `curl -N` for
   `/event-stream?id=1` and `client_id: 1` in the subscription RPC parameters, matching
   the existing KDF SSE handler.
8. Build a small native transfer via legacy withdraw and v2/task withdraw. Decode/check
   returned BOC and fees. Assert withdrawal alone did not advance seqno or move funds.
9. Under `--send`, select one fresh signed message, broadcast, then wait for its actual
   chain transaction and recipient result. Check balance/fees, seqno, history and events.
   Do not broadcast both alternative withdrawals built for the same seqno.
10. Retry the identical BOC and verify no second transfer. Test negative/malformed cases
    against mocks rather than intentionally spending fees on invalid mainnet messages.
11. Restart and assert history/cursor/pending recovery; verify a confirmed transfer is
    not counted twice. Unsubscribe and disable GRAM; stop KDF and verify task cleanup.

A tiny transfer back to the same funded address can exercise signing/broadcast, but
recipient-trace and accounting assertions must handle its asynchronous self-transfer.
Use disposable controlled testnet wallets for deployment, bounce, failure and broader
two-wallet cases. Do not assume the mainnet Iguana wallet is funded; report that gap,
and test its funded send path on testnet with a disposable fixture.

## 5. Required automated test matrix

| Layer | Required coverage | Uses user seed or money? |
| --- | --- | --- |
| Offline unit / cross-target | Keys/W5 vectors, addresses, decimals, ID bounds, BOC/signatures, expiry, fee math, errors | No; public disposable fixtures |
| Property/fuzz | Address/BOC parser bounds, no panic, amount round trips, stable IDs | No |
| Crypto regression | BIP39/Iguana unchanged, TON-only startup, encrypted reload, key-policy errors | No |
| Mock HTTP integration | Activation, provider error/failover/rate limits, fee/build/broadcast, ambiguous send, history/cursors | No |
| Local sandbox/emulator | W5 deployment, active transfer, compute/action failures, bounce behavior | No real funds |
| KDF RPC integration | Both envelopes; both modes; task lifecycle; single-address errors; history/streaming; shutdown | Public fixtures/mocks |
| Testnet network, feature-gated | Both modes with separate funded disposable wallets; external receipt/deployment/bounce | Disposable testnet funds |
| Private mainnet address check | Exact reference HD W5 from corrected numbered file | Reads seed locally; no transfer |
| Private mainnet read-only RPC | Balance, activation, history and subscriptions | Reads seed; no transfer |
| Private mainnet send | Small capped native transfer, actual outcome, history, fees, events, restart | Explicit `--send`; funded HD wallet |
| Coins config | JSON/schema/uniqueness, native platform classification, provider metadata and runtime loading | No |

Use `cross_test!` where this codebase expects native/WASM unit coverage and
`#[serde(deny_unknown_fields)]` on controlled test response types. Do not assert exact
live fees, timestamps or explorer ordering. Pin deterministic fee/state fixtures.
Never commit the funded mnemonic, signing key, private MM2.json, or API credentials.

Run focused tests after each milestone, then required project checks before completion:

```sh
cargo fmt --all -- --check
cargo test -p crypto --lib
cargo test -p coins --lib ton
cargo test --test mm2_tests_main ton_
cargo clippy --all-targets --all-features -- -D warnings
cargo test --bins --lib
cargo test --test mm2_tests_main
cargo build --release --bin kdf
```

Add feature forwarding for proposed `ton-network-tests` in coins/mm2_main and execute
those tests separately when credentials/funding exist. Check WASM according to
`docs/WASM_BUILD.md` and existing CI; test native-only runtime scripts on Linux.
Run dependency/license/security checks used by the repository and document any new
duplicates from `tonlib-core`. Check shell scripts with `bash -n` and ShellCheck if available.
Do not label an unavailable environment or unrun feature gate as a passed check.

## 6. Completion criteria and implementation report

- [ ] No production code path depends on the funded reference address or seed file.
- [ ] HD BIP39/SLIP-10 vector matches; Iguana has its own repeatable address; both key modes work.
- [ ] Unsupported extra addresses/accounts/scanning fail explicitly with no mutations.
- [ ] Real GRAM config loads from the modified coins repository.
- [ ] Legacy/v2 activation, balance, withdraw, broadcast and history are tested.
- [ ] Fees account for deployment and actual execution; unknown broadcast outcomes are
  reconciled without duplicate payment creation.
- [ ] History is durable and correct for messages, transactions, bounces and fees.
- [ ] Balance/history streaming is exercised, or any concrete remaining limitation is
  documented and not reported as implemented.
- [ ] Runtime directory contains built KDF, GRAM coins data, launch/test scripts and a
  reproducible manifest; the curl suite checks behavior and exits appropriately.
- [ ] Report exact tests run, pass/fail/skip counts, artifact location, source revisions,
  public mainnet message/transaction references and observed remaining balance.
- [ ] Update module documentation and this checklist as work progresses. Per root
  AGENTS, remove the active plan only after all required work is complete; preserve
  lasting API/runtime documentation outside the plan first.

## 7. Validation performed while preparing this plan

- Created the KDF integration branch from the clean local `dev` checkout.
- Read the applicable root/coin/activation/crypto/main AGENTS instructions, source
  call paths, TRX feature commits, ton-send implementation and coins config generator.
- Parsed `coins/coins` and checked relevant platform entries; GRAM/TON are absent.
- Inspected the locally cached `ton` 0.4.0 and `tonlib-core` 0.26.11 mnemonic/W5
  implementations and checked
  public TON documentation for derivation, wallet IDs, message lookup and provider APIs.
- Ran `cargo test --offline --locked --target-dir /tmp/kdf-ton-plan-reference-target`
  in `ton-send`: **5 tests passed**, none failed (3 library tests and 2 transfer tests).
- Checked the plan's Markdown fences, whitespace, required sections and absence of
  the seed phrase/numbered seed lines; confirmed that only the new plan is a KDF change.
- Revalidated the user-corrected numbered seed through `ton-rs` 0.4.0, then through
  `tonlib-core` 0.26.11. Passwordless
  TON validation passed and the derived mainnet W5R1 address exactly matched the HD
  reference (workchain 0, wallet ID 2147483409). A separate offline checksum check
  confirmed that the same phrase is not BIP39-valid. No secret material was printed.
  The verification helper and executable are local `/tmp/kdf-ton-verify-seed.rs` and
  `/tmp/kdf-ton-verify-seed`, linked against the previously built ton-send dependencies.
- Added initial KDF-local TON primitives for parsed/formatted addresses and W5R1 wallet
  construction. An isolated source-linked check ran **7 tests passed** (public native
  mnemonic vector, raw-seed determinism, subwallet bounds, raw/standard/URL-safe
  addresses and invalid checksum) and compiled for `wasm32-unknown-unknown`. Its
  disposable verifier read the corrected local seed without printing it and reported
  only a successful comparison with the public reference address.
- Added exact nanoGRAM parsing, formatting and checked arithmetic. Its isolated
  source-linked suite ran **3 tests passed**, covering exact 9-decimal conversion,
  invalid precision/format and `u64` bounds without rounding.
- Added a W5R1 transfer builder that creates the internal message and signed external
  BOC, includes StateInit only for a deploy transfer, and reports the external message
  cell hash distinctly from a transaction hash. The isolated source-linked suite now
  runs **14 tests passed**, including internal recipient/amount/bounce preservation,
  deploy BOC round trip, recipient network/bounce rejection, and a wasm32 check.
- Added the strict typed `CoinProtocol::TON` schema: explicit network, W5R1 version,
  workchain and bounded subwallet number. The protocol has no platform or contract
  address, rejects legacy pubkey-to-address derivation, and is deliberately rejected
  by legacy activation until the v2 TON activator is implemented. The source-linked
  suite now runs **17 tests passed**, including schema round trips and invalid
  version/subwallet/unknown-field checks, plus the wasm32 check.
- The initially implemented global `mnemonic_type: "ton"`, native TON key context,
  and `KeyPairPolicy::TonMnemonic` were removed after the derivation-policy review:
  they would prevent a native-TON-only phrase from deriving existing KDF HD coins.
  KDF now keeps its normal BIP39 startup for all coins.
- Added the matching `../coins` branch at `512ac11e`: native GRAM configuration,
  TON Center v2 node metadata, Tonscan explorer, and generator support for `ton/`.
  Validation confirms the schema and provider artifact without putting credentials in Git.
- Added an asynchronous TON Center v2 client for `getWalletInformation`. It validates
  the endpoint shape, sends dynamic API-key headers on native and WASM transports,
  bounds the request duration, preserves an absent active-account seqno as `None`, and
  parses nanoGRAM balances without floats. Its HTTP transport is not yet connected to
  activation; failover, retry/rate-limit policy, fees, broadcast and history remain M3–M7 work.
- Added TEP-3 multichain GRAM derivation at `m/44'/607'/0'` from the existing
  `GlobalHDAccountCtx`, with a public BIP39 seed→Ed25519 seed→non-bounceable W5R1
  test vector. The control BIP39 phrase is stored outside the repository in a
  mode-0600 file; only its public address is recorded above. The native TON reference
  address remains a non-KDF HD reference because its phrase does not pass BIP39.
- `cargo check --offline -p crypto --lib` reaches the existing `common` crate and stops
  on six `chrono` feature errors (`Utc::now`, `Local` and `DelayedFormat`) under the
  installed toolchain before `crypto` is checked. `cargo check --offline -p coins --lib`
  currently stops in the baseline `mm2_io`
  crate with 69 `std::io::Error: NotMmError` errors under the installed toolchain, before
  `coins` can be checked. `cargo fmt --all -- --check` likewise reports only the existing
  import order in `mm2src/derives/enum_derives/src/from_stringify.rs`. GRAM RPC tests,
  live balance verification and broadcast remain unimplemented; the `coins` repository
  has not yet been changed.
