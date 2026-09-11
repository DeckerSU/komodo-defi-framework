use super::{
    TonActivationError, TonActivationRequest, TonAddress, TonAddressFormat, TonAmount, TonCoinConfig, TonTxFeeDetails,
    TonWalletContext, TonWalletInformation, TON_DECIMALS,
};
use crate::coin_errors::{AddressFromPubkeyError, MyAddressError};
use crate::coin_errors::{ValidatePaymentError, ValidatePaymentResult};
use crate::hd_wallet::HDAddressSelector;
use crate::my_tx_history_v2::{CoinWithTxHistoryV2, MyTxHistoryErrorV2, MyTxHistoryTarget, TxHistoryStorage};
use crate::tx_history_storage::{GetTxHistoryFilters, TxHistoryStorageBuilder, WalletId};
use crate::utxo::utxo_common::big_decimal_from_sat_unsigned;
use crate::{
    BalanceError, BalanceFut, CheckIfMyPaymentSentArgs, CoinBalance, ConfirmPaymentInput, DexFee, FoundSwapTxSpend,
    HistorySyncState, MarketCoinOps, MmCoin, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy, RawTransactionError,
    RawTransactionFut, RawTransactionRequest, RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs,
    SignatureError, SignatureResult, SpendPaymentArgs, SwapOps, TradeFee, TradePreimageError, TradePreimageFut,
    TradePreimageResult, TradePreimageValue, TransactionData, TransactionDetails, TransactionErr, TransactionResult,
    TransactionType, TxFeeDetails, TxMarshalingErr, UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs,
    ValidateOtherPubKeyErr, ValidatePaymentInput, VerificationError, VerificationResult, WaitForHTLCTxSpendArgs,
    WatcherOps, WeakSpawner, WithdrawError, WithdrawFut, WithdrawRequest,
};
use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
    Engine as _,
};
use common::{
    executor::{abortable_queue::AbortableQueue, AbortableSystem, AbortedError, Timer},
    now_sec,
};
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use keys::KeyPair;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use parking_lot::Mutex;
use rpc::v1::types::Bytes as BytesJson;
use rpc::v1::types::H264 as H264Json;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Native GRAM wallet identity.
///
/// This intentionally has its own type instead of extending `EthCoin`: TON
/// addresses are W5 contract addresses and transfers are signed cell messages.
/// `MmCoin` integration is added separately once all mandatory wallet-only trait
/// methods have explicit implementations.
#[derive(Clone)]
pub struct TonCoin(Arc<TonCoinFields>);

struct TonCoinFields {
    wallet: TonWalletContext,
    abortable_system: AbortableQueue,
    required_confirmations: AtomicU64,
    pending_messages: Mutex<HashSet<String>>,
    pending_messages_path: Mutex<Option<PathBuf>>,
    history_sync_state: Mutex<HistorySyncState>,
}

const TON_SWAP_UNSUPPORTED: &str = "TON atomic swaps are not supported; GRAM is wallet-only";
const DEFAULT_TRANSFER_EXPIRATION_SECONDS: u64 = 60;
const MAX_TRANSFER_EXPIRATION_SECONDS: u64 = 3_600;
const ACCOUNT_TRANSACTION_LOOKBACK: u8 = 32;
const HISTORY_PAGE_SIZE: u8 = 100;
const MAX_HISTORY_PAGES_PER_SYNC: usize = 100;
const HISTORY_SYNC_INTERVAL_SECONDS: f64 = 30.0;

#[derive(Deserialize, Serialize)]
struct PersistedPendingMessages {
    message_hashes: Vec<String>,
}

fn unsupported_swap_transaction() -> TransactionResult {
    Err(TransactionErr::ProtocolNotSupported(TON_SWAP_UNSUPPORTED.to_owned()))
}

fn unsupported_swap_validation() -> ValidatePaymentResult<()> {
    MmError::err(ValidatePaymentError::InvalidParameter(TON_SWAP_UNSUPPORTED.to_owned()))
}

#[async_trait]
impl SwapOps for TonCoin {
    async fn send_taker_fee(&self, _: DexFee, _: &[u8], _: u64) -> TransactionResult {
        unsupported_swap_transaction()
    }
    async fn send_maker_payment(&self, _: SendPaymentArgs<'_>) -> TransactionResult {
        unsupported_swap_transaction()
    }
    async fn send_taker_payment(&self, _: SendPaymentArgs<'_>) -> TransactionResult {
        unsupported_swap_transaction()
    }
    async fn send_maker_spends_taker_payment(&self, _: SpendPaymentArgs<'_>) -> TransactionResult {
        unsupported_swap_transaction()
    }
    async fn send_taker_spends_maker_payment(&self, _: SpendPaymentArgs<'_>) -> TransactionResult {
        unsupported_swap_transaction()
    }
    async fn send_taker_refunds_payment(&self, _: RefundPaymentArgs<'_>) -> TransactionResult {
        unsupported_swap_transaction()
    }
    async fn send_maker_refunds_payment(&self, _: RefundPaymentArgs<'_>) -> TransactionResult {
        unsupported_swap_transaction()
    }
    async fn validate_fee(&self, _: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        unsupported_swap_validation()
    }
    async fn validate_maker_payment(&self, _: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        unsupported_swap_validation()
    }
    async fn validate_taker_payment(&self, _: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        unsupported_swap_validation()
    }
    async fn check_if_my_payment_sent(
        &self,
        _: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<crate::TransactionEnum>, String> {
        Err(TON_SWAP_UNSUPPORTED.to_owned())
    }
    async fn search_for_swap_tx_spend_my(
        &self,
        _: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        Err(TON_SWAP_UNSUPPORTED.to_owned())
    }
    async fn search_for_swap_tx_spend_other(
        &self,
        _: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        Err(TON_SWAP_UNSUPPORTED.to_owned())
    }
    async fn extract_secret(&self, _: &[u8], _: &[u8]) -> Result<[u8; 32], String> {
        Err(TON_SWAP_UNSUPPORTED.to_owned())
    }
    fn negotiate_swap_contract_addr(
        &self,
        _: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        MmError::err(NegotiateSwapContractAddrErr::NoOtherAddrAndNoFallback)
    }
    fn derive_htlc_key_pair(&self, _: &[u8]) -> KeyPair {
        KeyPair::default()
    }
    fn derive_htlc_pubkey(&self, _: &[u8]) -> [u8; 33] {
        [0; 33]
    }
    fn validate_other_pubkey(&self, _: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        MmError::err(ValidateOtherPubKeyErr::InvalidPubKey(TON_SWAP_UNSUPPORTED.to_owned()))
    }
}

#[async_trait]
impl MarketCoinOps for TonCoin {
    fn ticker(&self) -> &str {
        self.ticker()
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        let network = self.0.wallet.protocol().network;
        self.address()
            .map(|address| {
                address.format(
                    TonAddressFormat::Friendly {
                        bounceable: false,
                        urlsafe: true,
                    },
                    network,
                )
            })
            .map_err(|error| MyAddressError::InternalError(error.to_string()).into())
    }

    fn address_from_pubkey(&self, _pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        MmError::err(AddressFromPubkeyError::InternalError(
            "TON contract addresses cannot be derived from KDF's secp256k1 swap public key".to_owned(),
        ))
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        MmError::err(UnexpectedDerivationMethod::ExpectedSingleAddress)
    }

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        None
    }

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        MmError::err(SignatureError::InvalidRequest(
            "TON arbitrary-message signing is not supported".to_owned(),
        ))
    }

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        MmError::err(VerificationError::InvalidRequest(
            "TON arbitrary-message verification is not supported".to_owned(),
        ))
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let coin = self.clone();
        let future = async move {
            let information = coin
                .wallet_information()
                .await
                .map_err(|error| BalanceError::Transport(error.to_string()))?;
            Ok(CoinBalance::new(big_decimal_from_sat_unsigned(
                information.balance.as_nano(),
                TON_DECIMALS,
            )))
        };
        Box::new(future.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        Box::new(self.my_balance().map(|balance| balance.spendable))
    }

    fn platform_ticker(&self) -> &str {
        self.ticker()
    }

    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let tx = tx.strip_prefix("0x").unwrap_or(tx);
        let boc = match hex::decode(tx) {
            Ok(boc) => boc,
            Err(error) => return Box::new(futures01::future::err(format!("Invalid TON BOC hex: {error}"))),
        };
        self.send_raw_tx_bytes(&boc)
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let coin = self.clone();
        let boc = tx.to_vec();
        Box::new(
            async move {
                validate_external_boc(&boc)?;
                let message_hash = external_message_hash(&boc)?;
                coin.register_pending_message(&message_hash)?;
                if let Err(error) = coin.persist_pending_messages().await {
                    coin.remove_pending_message(&message_hash);
                    return Err(error);
                }
                match coin.0.wallet.broadcast_boc(&boc).await {
                    Ok(reference) => Ok(reference),
                    Err(error) => {
                        // A timeout or transport failure can occur after the
                        // provider accepted the BOC, so retain this entry for
                        // later reconciliation and never sign a replacement.
                        if !matches!(
                            error,
                            TonActivationError::Rpc(super::TonRpcError::Timeout | super::TonRpcError::Transport)
                        ) {
                            coin.remove_pending_message(&message_hash);
                            coin.persist_pending_messages().await?;
                        }
                        Err(error.to_string())
                    },
                }
            }
            .boxed()
            .compat(),
        )
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        let message_hash = try_fus!(external_message_hash(&input.payment_tx));
        if input.confirmations == 0 || input.requires_nota {
            return Box::new(futures01::future::err("TON does not support notarization".to_owned()));
        }
        if input.check_every == 0 {
            return Box::new(futures01::future::err(
                "TON confirmation interval must be greater than zero".to_owned(),
            ));
        }
        let coin = self.clone();
        Box::new(
            async move {
                loop {
                    let transactions = coin
                        .account_transactions(ACCOUNT_TRANSACTION_LOOKBACK)
                        .await
                        .map_err(|error| error.to_string())?;
                    if transactions.iter().any(|transaction| {
                        transaction
                            .inbound_message_hash
                            .as_deref()
                            .map_or(false, |hash| ton_hashes_equal(&message_hash, hash))
                    }) {
                        if let Some(outcome) = coin
                            .0
                            .wallet
                            .message_outcome(&message_hash)
                            .await
                            .map_err(|error| error.to_string())?
                        {
                            if outcome.compute_success != Some(true)
                                || outcome.action_success != Some(true)
                                || outcome.recipient_bounced
                            {
                                return Err(
                                    "TON message was included but execution or recipient delivery failed".to_owned()
                                );
                            }
                            let transfer_messages: Vec<_> = outcome
                                .outbound_messages
                                .iter()
                                .filter(|message| message.value != TonAmount::ZERO)
                                .collect();
                            if transfer_messages.is_empty() {
                                return Err(
                                    "TON wallet transaction created no value-carrying outgoing message".to_owned()
                                );
                            }
                            let mut all_recipients_succeeded = true;
                            for message in transfer_messages {
                                let recipient = coin
                                    .0
                                    .wallet
                                    .message_outcome(&message.hash)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                match recipient {
                                    Some(recipient) if recipient.transaction_hash != outcome.transaction_hash => {
                                        if recipient.compute_success == Some(false)
                                            || recipient.action_success == Some(false)
                                            || recipient.recipient_bounced
                                        {
                                            return Err(
                                                "TON recipient transaction reported failed execution or a bounce"
                                                    .to_owned(),
                                            );
                                        }
                                    },
                                    _ => all_recipients_succeeded = false,
                                }
                            }
                            if !all_recipients_succeeded {
                                if now_sec() >= input.wait_until {
                                    return Err("Timed out waiting for TON recipient transaction execution".to_owned());
                                }
                                Timer::sleep(input.check_every as f64).await;
                                continue;
                            }
                            let included_at = outcome.masterchain_seqno;
                            let current = coin.current_block_number().await.map_err(|error| error.to_string())?;
                            let required = included_at
                                .checked_add(input.confirmations - 1)
                                .ok_or_else(|| "TON confirmation height overflow".to_owned())?;
                            if current >= required {
                                coin.remove_pending_message(&message_hash);
                                coin.persist_pending_messages().await?;
                                return Ok(());
                            }
                        }
                    }
                    if now_sec() >= input.wait_until {
                        return Err("Timed out waiting for TON account transaction inclusion".to_owned());
                    }
                    Timer::sleep(input.check_every as f64).await;
                }
            }
            .boxed()
            .compat(),
        )
    }

    async fn wait_for_htlc_tx_spend(&self, _args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        Err(TransactionErr::ProtocolNotSupported(
            "TON atomic swaps are not supported".to_owned(),
        ))
    }

    fn tx_enum_from_bytes(&self, _bytes: &[u8]) -> Result<crate::TransactionEnum, MmError<TxMarshalingErr>> {
        MmError::err(TxMarshalingErr::NotSupported(
            "TON transaction decoding is not implemented".to_owned(),
        ))
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        let coin = self.clone();
        Box::new(
            async move { coin.0.wallet.current_block().await.map_err(|error| error.to_string()) }
                .boxed()
                .compat(),
        )
    }

    fn display_priv_key(&self) -> Result<String, String> {
        Err("TON private-key export is not supported".to_owned())
    }

    fn min_tx_amount(&self) -> BigDecimal {
        big_decimal_from_sat_unsigned(1, TON_DECIMALS)
    }

    fn min_trading_vol(&self) -> MmNumber {
        big_decimal_from_sat_unsigned(1, TON_DECIMALS).into()
    }

    fn should_burn_dex_fee(&self) -> bool {
        false
    }

    fn is_trezor(&self) -> bool {
        false
    }
}

// TON is wallet-only, so all watcher methods use the trait's explicit
// unsupported-operation defaults.
#[async_trait]
impl WatcherOps for TonCoin {}

#[async_trait]
impl MmCoin for TonCoin {
    fn is_asset_chain(&self) -> bool {
        false
    }
    fn wallet_only(&self, _ctx: &mm2_core::mm_ctx::MmArc) -> bool {
        true
    }
    fn spawner(&self) -> WeakSpawner {
        self.0.abortable_system.weak_spawner()
    }
    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        let coin = self.clone();
        Box::new(async move { coin.build_withdraw(req).await }.boxed().compat())
    }
    fn get_raw_transaction(&self, _req: RawTransactionRequest) -> RawTransactionFut<'_> {
        Box::new(futures01::future::err(MmError::new(
            RawTransactionError::NotImplemented {
                coin: self.ticker().to_owned(),
            },
        )))
    }
    fn get_tx_hex_by_hash(&self, _tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        Box::new(futures01::future::err(MmError::new(
            RawTransactionError::NotImplemented {
                coin: self.ticker().to_owned(),
            },
        )))
    }
    fn decimals(&self) -> u8 {
        TON_DECIMALS
    }
    fn convert_to_address(&self, from: &str, to: serde_json::Value) -> Result<String, String> {
        let address = TonAddress::parse(from).map_err(|error| error.to_string())?;
        let format = serde_json::from_value(to).map_err(|_| "Invalid TON address format".to_owned())?;
        Ok(address.format(format, self.0.wallet.protocol().network))
    }
    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        match TonAddress::parse(address).and_then(|address| address.ensure_network(self.0.wallet.protocol().network)) {
            Ok(_) => ValidateAddressResult {
                is_valid: true,
                reason: None,
            },
            Err(error) => ValidateAddressResult {
                is_valid: false,
                reason: Some(error.to_string()),
            },
        }
    }
    fn process_history_loop(&self, ctx: mm2_core::mm_ctx::MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let storage = match TxHistoryStorageBuilder::new(&ctx).build() {
            Ok(storage) => storage,
            Err(error) => {
                self.set_history_sync_state(HistorySyncState::Error(serde_json::json!({
                    "message": format!("TON history storage initialization failed: {error}"),
                })));
                return Box::new(futures01::future::ok(()));
            },
        };
        let coin = self.clone();
        Box::new(
            async move {
                coin.history_loop(storage).await;
                Ok(())
            }
            .boxed()
            .compat(),
        )
    }
    fn history_sync_status(&self) -> HistorySyncState {
        self.0.history_sync_state.lock().clone()
    }
    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        Box::new(futures01::future::err(TON_SWAP_UNSUPPORTED.to_owned()))
    }
    async fn get_sender_trade_fee(
        &self,
        _: TradePreimageValue,
        _: crate::FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        MmError::err(TradePreimageError::ProtocolNotSupported(
            TON_SWAP_UNSUPPORTED.to_owned(),
        ))
    }
    fn get_receiver_trade_fee(&self, _: crate::FeeApproxStage) -> TradePreimageFut<TradeFee> {
        Box::new(
            futures::future::ready(MmError::err(TradePreimageError::ProtocolNotSupported(
                TON_SWAP_UNSUPPORTED.to_owned(),
            )))
            .compat(),
        )
    }
    async fn get_fee_to_send_taker_fee(&self, _: DexFee, _: crate::FeeApproxStage) -> TradePreimageResult<TradeFee> {
        MmError::err(TradePreimageError::ProtocolNotSupported(
            TON_SWAP_UNSUPPORTED.to_owned(),
        ))
    }
    fn required_confirmations(&self) -> u64 {
        self.0.required_confirmations.load(Ordering::Relaxed)
    }
    fn requires_notarization(&self) -> bool {
        false
    }
    fn set_required_confirmations(&self, confirmations: u64) {
        if confirmations > 0 {
            self.0.required_confirmations.store(confirmations, Ordering::Relaxed);
        }
    }
    fn set_requires_notarization(&self, _: bool) {}
    fn swap_contract_address(&self) -> Option<BytesJson> {
        None
    }
    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        None
    }
    fn mature_confirmations(&self) -> Option<u32> {
        None
    }
    fn coin_protocol_info(&self, _: Option<MmNumber>) -> Vec<u8> {
        Vec::new()
    }
    fn is_coin_protocol_supported(&self, _: &Option<Vec<u8>>, _: Option<MmNumber>, _: u64, _: bool) -> bool {
        false
    }
    fn on_disabled(&self) -> Result<(), AbortedError> {
        self.0.abortable_system.abort_all()
    }
    fn on_token_deactivated(&self, _: &str) {}
}

impl TonCoin {
    pub fn new(
        config: TonCoinConfig,
        request: TonActivationRequest,
        key_policy: PrivKeyBuildPolicy,
    ) -> Result<Self, TonActivationError> {
        let wallet = TonWalletContext::new(config, request, key_policy)?;
        let required_confirmations = wallet.required_confirmations();
        let history_enabled = wallet.tx_history_enabled();
        Ok(TonCoin(Arc::new(TonCoinFields {
            wallet,
            abortable_system: AbortableQueue::default(),
            required_confirmations: AtomicU64::new(required_confirmations),
            pending_messages: Mutex::new(HashSet::new()),
            pending_messages_path: Mutex::new(None),
            history_sync_state: Mutex::new(if history_enabled {
                HistorySyncState::NotStarted
            } else {
                HistorySyncState::NotEnabled
            }),
        })))
    }

    /// Builds the coin and verifies that its account state can safely be used
    /// for a future W5 transfer before it is registered in `CoinsContext`.
    pub async fn activate(
        config: TonCoinConfig,
        request: TonActivationRequest,
        key_policy: PrivKeyBuildPolicy,
    ) -> Result<Self, TonActivationError> {
        let coin = TonCoin::new(config, request, key_policy)?;
        coin.0.wallet.validate_account_state().await?;
        Ok(coin)
    }

    /// Activates a KDF-managed TON wallet and restores its unresolved external
    /// message tracker before any new BOC can be broadcast.
    pub async fn activate_with_context(
        ctx: &mm2_core::mm_ctx::MmArc,
        config: TonCoinConfig,
        request: TonActivationRequest,
        key_policy: PrivKeyBuildPolicy,
    ) -> Result<Self, TonActivationError> {
        let coin = TonCoin::new(config, request, key_policy)?;
        coin.initialize_pending_message_store(ctx).await?;
        coin.0.wallet.validate_account_state().await?;
        // Failure to query the newest account page must not discard an exact
        // persisted message reference or make activation unavailable.
        let _ = coin.reconcile_pending_messages().await;
        Ok(coin)
    }

    pub fn ticker(&self) -> &str {
        self.0.wallet.ticker()
    }

    pub fn address(&self) -> Result<TonAddress, TonActivationError> {
        self.0.wallet.address()
    }

    pub fn required_confirmations(&self) -> u64 {
        self.0.wallet.required_confirmations()
    }

    pub fn tx_history_enabled(&self) -> bool {
        self.0.wallet.tx_history_enabled()
    }

    pub async fn wallet_information(&self) -> Result<TonWalletInformation, TonActivationError> {
        self.0.wallet.wallet_information().await
    }

    pub async fn current_block_number(&self) -> Result<u64, TonActivationError> {
        self.0.wallet.current_block().await
    }

    pub async fn account_transactions(
        &self,
        limit: u8,
    ) -> Result<Vec<super::TonAccountTransaction>, TonActivationError> {
        self.0.wallet.account_transactions(limit).await
    }

    pub fn history_wallet_id(&self) -> WalletId {
        WalletId::new(self.ticker().to_owned())
    }

    fn set_history_sync_state(&self, state: HistorySyncState) {
        *self.0.history_sync_state.lock() = state;
    }

    /// Fetches a bounded TON Center v2 history snapshot and stores only account
    /// transactions whose serialized BOC is present. A provider that omits the
    /// BOC cannot produce a truthful `TransactionData::Signed` record.
    pub async fn sync_history_once<Storage>(&self, storage: &Storage) -> Result<usize, String>
    where
        Storage: TxHistoryStorage,
    {
        let wallet_id = self.history_wallet_id();
        storage
            .init(&wallet_id)
            .await
            .map_err(|error| format!("TON history storage initialization failed: {error:?}"))?;

        let my_address = self.my_address().map_err(|error| error.to_string())?;
        let mut cursor = None;
        let mut added = 0;
        for _ in 0..MAX_HISTORY_PAGES_PER_SYNC {
            let transactions = self
                .0
                .wallet
                .account_transactions_page(HISTORY_PAGE_SIZE, cursor.as_ref())
                .await
                .map_err(|error| error.to_string())?;
            if transactions.is_empty() {
                break;
            }

            let page_len = transactions.len();
            let next_cursor = transactions.last().map(super::TonAccountTransaction::cursor);
            let mut new_transactions = Vec::new();
            for transaction in transactions {
                let Some(details) = self
                    .transaction_details_from_account_transaction(&my_address, transaction)
                    .await?
                else {
                    continue;
                };
                let tx_hash = details
                    .tx
                    .tx_hash()
                    .ok_or_else(|| "TON history details are missing the transaction hash".to_owned())?;
                if !storage
                    .history_has_tx_hash(&wallet_id, tx_hash)
                    .await
                    .map_err(|error| format!("TON history storage lookup failed: {error:?}"))?
                {
                    new_transactions.push(details);
                }
            }
            added += new_transactions.len();
            if !new_transactions.is_empty() {
                storage
                    .add_transactions_to_history(&wallet_id, new_transactions)
                    .await
                    .map_err(|error| format!("TON history storage write failed: {error:?}"))?;
            }
            if page_len < usize::from(HISTORY_PAGE_SIZE) {
                break;
            }
            cursor = next_cursor;
        }
        Ok(added)
    }

    pub async fn history_loop<Storage>(&self, storage: Storage)
    where
        Storage: TxHistoryStorage,
    {
        loop {
            self.set_history_sync_state(HistorySyncState::InProgress(serde_json::json!({})));
            match self.sync_history_once(&storage).await {
                Ok(_) => self.set_history_sync_state(HistorySyncState::Finished),
                Err(error) => {
                    self.set_history_sync_state(HistorySyncState::Error(serde_json::json!({ "message": error })));
                    return;
                },
            }
            Timer::sleep(HISTORY_SYNC_INTERVAL_SECONDS).await;
        }
    }

    async fn transaction_details_from_account_transaction(
        &self,
        my_address: &str,
        transaction: super::TonAccountTransaction,
    ) -> Result<Option<TransactionDetails>, String> {
        let Some(boc) = transaction.boc else { return Ok(None) };
        let internal_id = ton_hash_bytes(&transaction.hash)?;
        let inbound_height = match transaction.inbound_message_hash.as_deref() {
            Some(message_hash) => self
                .0
                .wallet
                .message_outcome(message_hash)
                .await
                .map_err(|error| error.to_string())?
                .map(|outcome| outcome.masterchain_seqno),
            None => None,
        };

        let mut from = HashSet::new();
        let mut to = HashSet::new();
        let mut received = TonAmount::ZERO;
        if let Some(inbound) = transaction.inbound_message {
            if let Some(source) = inbound.source {
                from.insert(source);
                if inbound
                    .destination
                    .as_deref()
                    .map_or(false, |destination| ton_addresses_equal(destination, my_address))
                {
                    received = inbound.value;
                }
            }
            if let Some(destination) = inbound.destination {
                to.insert(destination);
            }
        }

        let mut transferred = TonAmount::ZERO;
        for outbound in transaction.outbound_messages {
            if let Some(source) = outbound.source {
                from.insert(source);
            } else {
                from.insert(my_address.to_owned());
            }
            if let Some(destination) = outbound.destination {
                to.insert(destination);
            }
            transferred = transferred
                .checked_add(outbound.value)
                .map_err(|error| error.to_string())?;
        }
        let total_amount = received.checked_add(transferred).map_err(|error| error.to_string())?;
        let spent = transferred
            .checked_add(transaction.fee)
            .map_err(|error| error.to_string())?;
        let received_decimal = big_decimal_from_sat_unsigned(received.as_nano(), TON_DECIMALS);
        let spent_decimal = big_decimal_from_sat_unsigned(spent.as_nano(), TON_DECIMALS);

        let mut from: Vec<_> = from.into_iter().collect();
        let mut to: Vec<_> = to.into_iter().collect();
        from.sort();
        to.sort();
        Ok(Some(TransactionDetails {
            tx: TransactionData::new_signed(BytesJson(boc), transaction.hash),
            from,
            to,
            total_amount: big_decimal_from_sat_unsigned(total_amount.as_nano(), TON_DECIMALS),
            spent_by_me: spent_decimal.clone(),
            received_by_me: received_decimal.clone(),
            my_balance_change: received_decimal - spent_decimal,
            block_height: inbound_height.unwrap_or_default(),
            timestamp: transaction.timestamp,
            fee_details: Some(TxFeeDetails::Ton(TonTxFeeDetails::from_actual_fee(
                self.ticker().to_owned(),
                transaction.fee.as_nano(),
            ))),
            coin: self.ticker().to_owned(),
            internal_id: BytesJson(internal_id),
            kmd_rewards: None,
            transaction_type: TransactionType::StandardTransfer,
            memo: None,
        }))
    }

    fn register_pending_message(&self, message_hash: &str) -> Result<(), String> {
        let mut pending = self.0.pending_messages.lock();
        if !pending.is_empty() {
            return Err(
                "TON wallet has an unresolved external message; reconcile it before sending another transfer"
                    .to_owned(),
            );
        }
        pending.insert(message_hash.to_owned());
        Ok(())
    }

    fn remove_pending_message(&self, message_hash: &str) {
        self.0.pending_messages.lock().remove(message_hash);
    }

    async fn initialize_pending_message_store(&self, ctx: &mm2_core::mm_ctx::MmArc) -> Result<(), TonActivationError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let address = self
                .my_address()
                .map_err(|error| TonActivationError::PendingMessagePersistence(error.to_string()))?;
            let path = ctx
                .dbdir()
                .join("TON_PENDING")
                .join(format!("{}_{}.json", self.ticker(), address));
            let messages = load_persisted_pending_messages(&path)
                .await
                .map_err(TonActivationError::PendingMessagePersistence)?;
            if messages.len() > 1 || messages.iter().any(|hash| hash.is_empty() || hash.len() > 256) {
                return Err(TonActivationError::PendingMessagePersistence(
                    "invalid persisted TON pending-message tracker".to_owned(),
                ));
            }
            *self.0.pending_messages.lock() = messages.into_iter().collect();
            *self.0.pending_messages_path.lock() = Some(path);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
        Ok(())
    }

    async fn persist_pending_messages(&self) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.0.pending_messages_path.lock().clone();
            let Some(path) = path else { return Ok(()) };
            let message_hashes = self.0.pending_messages.lock().iter().cloned().collect();
            let content = serde_json::to_vec(&PersistedPendingMessages { message_hashes })
                .map_err(|error| format!("could not serialize TON pending-message tracker: {error}"))?;
            let parent = path
                .parent()
                .ok_or_else(|| "TON pending-message tracker has no parent directory".to_owned())?;
            async_std::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("could not create TON pending-message directory: {error}"))?;
            let temporary = path.with_extension("tmp");
            async_std::fs::write(&temporary, content)
                .await
                .map_err(|error| format!("could not write TON pending-message tracker: {error}"))?;
            async_std::fs::rename(&temporary, path)
                .await
                .map_err(|error| format!("could not replace TON pending-message tracker: {error}"))?;
        }
        Ok(())
    }

    /// Removes locally pending external messages once their exact inbound
    /// message hash appears in the account's newest transactions. This is
    /// inclusion reconciliation only: it does not claim recipient execution
    /// or masterchain confirmation.
    pub async fn reconcile_pending_messages(&self) -> Result<usize, TonActivationError> {
        let transactions = self.0.wallet.account_transactions(ACCOUNT_TRANSACTION_LOOKBACK).await?;
        let observed: Vec<_> = transactions
            .iter()
            .filter_map(|transaction| transaction.inbound_message_hash.as_deref())
            .collect();
        let removed = {
            let mut pending = self.0.pending_messages.lock();
            let before = pending.len();
            pending.retain(|pending_hash| !observed.iter().any(|hash| ton_hashes_equal(pending_hash, hash)));
            before - pending.len()
        };
        if removed != 0 {
            self.persist_pending_messages()
                .await
                .map_err(TonActivationError::PendingMessagePersistence)?;
        }
        Ok(removed)
    }

    async fn build_withdraw(&self, req: WithdrawRequest) -> Result<TransactionDetails, MmError<WithdrawError>> {
        ensure_default_sender(req.from.as_ref())?;
        if req.fee.is_some() {
            return MmError::err(WithdrawError::InvalidFeePolicy(
                "manual TON fees are not supported; omit the fee field".to_owned(),
            ));
        }
        if req.memo.is_some() {
            return MmError::err(WithdrawError::InvalidMemo(
                "TON transfer comments are not implemented".to_owned(),
            ));
        }
        if req.broadcast {
            return MmError::err(WithdrawError::UnsupportedError(
                "TON withdraw returns a signed BOC; submit it with send_raw_transaction".to_owned(),
            ));
        }

        let recipient = TonAddress::parse(&req.to).map_err(|error| WithdrawError::InvalidAddress(error.to_string()))?;
        recipient
            .ensure_network(self.0.wallet.protocol().network)
            .map_err(|error| WithdrawError::InvalidAddress(error.to_string()))?;
        let expire_at = transfer_expire_at(req.expiration_seconds)?;
        let prepared = if req.max {
            self.prepare_max_transfer(recipient.clone(), expire_at).await?
        } else {
            let amount = req
                .amount
                .to_string()
                .parse::<TonAmount>()
                .map_err(|error| WithdrawError::InvalidFee {
                    reason: format!("invalid GRAM amount: {error}"),
                    details: None,
                })?;
            if amount == TonAmount::ZERO {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: req.amount,
                    threshold: self.min_tx_amount(),
                });
            }
            self.0
                .wallet
                .prepare_transfer(recipient.clone(), amount, expire_at)
                .await
                .map_err(ton_withdraw_error)?
        };

        let source_fee = prepared.fee.source_total().map_err(ton_rpc_withdraw_error)?;
        let required = prepared
            .amount
            .checked_add(TonAmount::from_nano(source_fee))
            .map_err(|error| WithdrawError::InternalError(error.to_string()))?;
        if prepared.available_balance < required {
            return MmError::err(WithdrawError::NotSufficientBalance {
                coin: self.ticker().to_owned(),
                available: big_decimal_from_sat_unsigned(prepared.available_balance.as_nano(), TON_DECIMALS),
                required: big_decimal_from_sat_unsigned(required.as_nano(), TON_DECIMALS),
            });
        }

        let my_address = self
            .my_address()
            .map_err(|error| WithdrawError::InternalError(error.to_string()))?;
        let recipient_address = recipient.format(
            TonAddressFormat::Friendly {
                bounceable: false,
                urlsafe: true,
            },
            self.0.wallet.protocol().network,
        );
        let amount_decimal = big_decimal_from_sat_unsigned(prepared.amount.as_nano(), TON_DECIMALS);
        let fee_details =
            TonTxFeeDetails::from_estimate(self.ticker().to_owned(), &prepared.fee).map_err(ton_rpc_withdraw_error)?;
        let total_fee = fee_details.total_fee.clone();
        let received_by_me = if recipient_address == my_address {
            amount_decimal.clone()
        } else {
            0.into()
        };
        let spent_by_me = &amount_decimal + &total_fee;

        Ok(TransactionDetails {
            tx: TransactionData::new_signed(BytesJson(prepared.signed.boc), prepared.signed.message_hash.clone()),
            from: vec![my_address],
            to: vec![recipient_address],
            total_amount: amount_decimal,
            spent_by_me: spent_by_me.clone(),
            received_by_me: received_by_me.clone(),
            my_balance_change: received_by_me - spent_by_me,
            block_height: 0,
            timestamp: now_sec(),
            fee_details: Some(TxFeeDetails::Ton(fee_details)),
            coin: req.coin,
            internal_id: BytesJson(prepared.signed.message_hash.into_bytes()),
            kmd_rewards: None,
            transaction_type: TransactionType::StandardTransfer,
            memo: None,
        })
    }

    async fn prepare_max_transfer(
        &self,
        recipient: TonAddress,
        expire_at: u32,
    ) -> Result<super::TonPreparedTransfer, MmError<WithdrawError>> {
        let information = self.0.wallet.wallet_information().await.map_err(ton_withdraw_error)?;
        if information.balance == TonAmount::ZERO {
            return MmError::err(WithdrawError::ZeroBalanceToWithdrawMax);
        }

        let mut prepared = self
            .0
            .wallet
            .prepare_transfer(recipient.clone(), information.balance, expire_at)
            .await
            .map_err(ton_withdraw_error)?;
        for _ in 0..3 {
            let source_fee = prepared.fee.source_total().map_err(ton_rpc_withdraw_error)?;
            let amount = prepared
                .available_balance
                .checked_sub(TonAmount::from_nano(source_fee))
                .map_err(|_| {
                    MmError::new(WithdrawError::AmountTooLow {
                        amount: 0.into(),
                        threshold: big_decimal_from_sat_unsigned(source_fee, TON_DECIMALS),
                    })
                })?;
            if amount == TonAmount::ZERO {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: 0.into(),
                    threshold: big_decimal_from_sat_unsigned(source_fee, TON_DECIMALS),
                });
            }
            let next = self
                .0
                .wallet
                .prepare_transfer(recipient.clone(), amount, expire_at)
                .await
                .map_err(ton_withdraw_error)?;
            let next_fee = next.fee.source_total().map_err(ton_rpc_withdraw_error)?;
            if next.available_balance == prepared.available_balance && next_fee == source_fee {
                return Ok(next);
            }
            prepared = next;
        }
        MmError::err(WithdrawError::InvalidFee {
            reason: "TON max-withdraw fee estimate did not converge".to_owned(),
            details: None,
        })
    }
}

#[async_trait]
impl CoinWithTxHistoryV2 for TonCoin {
    fn history_wallet_id(&self) -> WalletId {
        TonCoin::history_wallet_id(self)
    }

    async fn get_tx_history_filters(
        &self,
        target: MyTxHistoryTarget,
    ) -> MmResult<GetTxHistoryFilters, MyTxHistoryErrorV2> {
        match target {
            MyTxHistoryTarget::Iguana | MyTxHistoryTarget::AccountId { account_id: 0 } => {
                Ok(GetTxHistoryFilters::for_address(self.my_address().map_mm_err()?))
            },
            MyTxHistoryTarget::AddressId(path)
                if path.account_id == 0 && path.address_id == 0 && path.chain == crypto::Bip44Chain::External =>
            {
                Ok(GetTxHistoryFilters::for_address(self.my_address().map_mm_err()?))
            },
            target => MmError::err(MyTxHistoryErrorV2::with_expected_target(
                target,
                "the fixed primary TON address",
            )),
        }
    }
}

fn ensure_default_sender(sender: Option<&HDAddressSelector>) -> Result<(), MmError<WithdrawError>> {
    match sender {
        None => Ok(()),
        Some(HDAddressSelector::AddressId(path))
            if path.account_id == 0 && path.address_id == 0 && path.chain == crypto::Bip44Chain::External =>
        {
            Ok(())
        },
        Some(_) => MmError::err(WithdrawError::UnexpectedFromAddress(
            "GRAM supports only its fixed primary TON address".to_owned(),
        )),
    }
}

fn transfer_expire_at(expiration_seconds: Option<u64>) -> Result<u32, MmError<WithdrawError>> {
    let expiration_seconds = expiration_seconds.unwrap_or(DEFAULT_TRANSFER_EXPIRATION_SECONDS);
    if expiration_seconds == 0 || expiration_seconds > MAX_TRANSFER_EXPIRATION_SECONDS {
        return MmError::err(WithdrawError::InvalidFee {
            reason: format!("TON expiration_seconds must be between 1 and {MAX_TRANSFER_EXPIRATION_SECONDS} seconds"),
            details: None,
        });
    }
    now_sec()
        .checked_add(expiration_seconds)
        .and_then(|timestamp| (timestamp <= u64::from(u32::MAX)).then_some(timestamp as u32))
        .ok_or_else(|| {
            MmError::new(WithdrawError::InternalError(
                "TON transfer expiration cannot be represented".to_owned(),
            ))
        })
}

fn ton_withdraw_error(error: TonActivationError) -> MmError<WithdrawError> {
    MmError::new(WithdrawError::Transport(error.to_string()))
}

fn ton_rpc_withdraw_error(error: super::TonRpcError) -> MmError<WithdrawError> {
    MmError::new(WithdrawError::Transport(error.to_string()))
}

fn validate_external_boc(boc: &[u8]) -> Result<(), String> {
    use tonlib_core::tlb_types::{
        block::message::{CommonMsgInfo, Message},
        tlb::TLB,
    };

    let message = Message::from_boc(boc).map_err(|_| "Invalid TON external-message BOC".to_owned())?;
    if !matches!(message.info, CommonMsgInfo::ExtIn(_)) {
        return Err("TON raw BOC must contain an external incoming message".to_owned());
    }
    Ok(())
}

fn external_message_hash(boc: &[u8]) -> Result<String, String> {
    use tonlib_core::tlb_types::{block::message::Message, tlb::TLB};

    Message::from_boc(boc)
        .and_then(|message| message.cell_hash())
        .map(|hash| hash.to_hex())
        .map_err(|_| "Invalid TON external-message BOC".to_owned())
}

fn ton_hashes_equal(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let decode = |value: &str| {
        hex::decode(value)
            .ok()
            .or_else(|| BASE64.decode(value).ok())
            .or_else(|| URL_SAFE_NO_PAD.decode(value).ok())
    };
    matches!((decode(left), decode(right)), (Some(left), Some(right)) if left == right)
}

fn ton_hash_bytes(hash: &str) -> Result<Vec<u8>, String> {
    hex::decode(hash)
        .ok()
        .or_else(|| BASE64.decode(hash).ok())
        .or_else(|| URL_SAFE_NO_PAD.decode(hash).ok())
        .filter(|bytes| !bytes.is_empty() && bytes.len() <= 256)
        .ok_or_else(|| "TON provider returned an invalid transaction hash".to_owned())
}

fn ton_addresses_equal(left: &str, right: &str) -> bool {
    match (TonAddress::parse(left), TonAddress::parse(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn load_persisted_pending_messages(path: &std::path::Path) -> Result<Vec<String>, String> {
    match async_std::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice::<PersistedPendingMessages>(&bytes)
            .map(|messages| messages.message_hashes)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ton::{build_signed_transfer, TonNetwork, TonTransferRequest, TonWalletParams},
        IguanaPrivKey, PrivKeyBuildPolicy,
    };
    use serde_json::json;

    #[cfg(not(target_arch = "wasm32"))]
    use common::block_on;

    fn config() -> TonCoinConfig {
        TonCoinConfig::from_json(json!({
            "coin": "GRAM", "decimals": 9, "required_confirmations": 1, "wallet_only": true,
            "protocol": {"type": "TON", "protocol_data": {
                "network": "Mainnet", "wallet_version": "V5R1", "workchain": 0, "subwallet_number": 0
            }}
        }))
        .unwrap()
    }

    fn request() -> TonActivationRequest {
        serde_json::from_value(json!({"nodes":[{"url":"https://toncenter.com/api/v2"}]})).unwrap()
    }

    #[test]
    fn coin_keeps_the_validated_wallet_identity() {
        let coin = TonCoin::new(
            config(),
            request(),
            PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from([0x42; 32])),
        )
        .unwrap();

        assert_eq!(coin.ticker(), "GRAM");
        assert_eq!(coin.required_confirmations(), 1);
        assert!(!coin.tx_history_enabled());
        assert!(coin.address().is_ok());
    }

    #[test]
    fn accepts_only_a_signed_external_message_boc_for_broadcast() {
        let wallet = TonWalletParams::MAINNET_DEFAULT.wallet_from_seed(&[0x42; 32]).unwrap();
        let transfer = build_signed_transfer(
            &wallet,
            TonNetwork::Mainnet,
            &TonTransferRequest {
                recipient: TonAddress::parse("UQBYGTsWwxh00p3Fq_EdwzQ2uRzuptfxP5crEOsfRT6zDOS4").unwrap(),
                amount: "0.001".parse().unwrap(),
                bounceable: false,
                sequence_number: 0,
                expire_at: 1_700_000_000,
                deploy_wallet: true,
            },
        )
        .unwrap();

        assert!(validate_external_boc(&transfer.boc).is_ok());
        assert_eq!(external_message_hash(&transfer.boc).unwrap(), transfer.message_hash);
        assert!(validate_external_boc(&[0, 1, 2]).is_err());
    }

    #[test]
    fn accepts_only_the_fixed_primary_address_selector() {
        use crate::hd_wallet::HDPathAccountToAddressId;
        use crypto::Bip44Chain;

        assert!(ensure_default_sender(None).is_ok());
        assert!(
            ensure_default_sender(Some(&HDAddressSelector::AddressId(HDPathAccountToAddressId::default()))).is_ok()
        );
        assert!(
            ensure_default_sender(Some(&HDAddressSelector::AddressId(HDPathAccountToAddressId {
                account_id: 0,
                chain: Bip44Chain::External,
                address_id: 1,
            })))
            .is_err()
        );
    }

    #[test]
    fn bounds_transfer_expiration() {
        assert!(transfer_expire_at(None).is_ok());
        assert!(transfer_expire_at(Some(1)).is_ok());
        assert!(transfer_expire_at(Some(0)).is_err());
        assert!(transfer_expire_at(Some(MAX_TRANSFER_EXPIRATION_SECONDS + 1)).is_err());
    }

    #[test]
    fn compares_provider_and_local_message_hash_encodings() {
        let raw = [0xabu8; 32];
        assert!(ton_hashes_equal(&hex::encode(raw), &BASE64.encode(raw)));
        assert!(ton_hashes_equal(&hex::encode(raw), &URL_SAFE_NO_PAD.encode(raw)));
        assert!(!ton_hashes_equal(&hex::encode(raw), &hex::encode([0xcdu8; 32])));
    }

    #[test]
    fn prevents_parallel_external_messages_for_one_wallet() {
        let coin = TonCoin::new(
            config(),
            request(),
            PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from([0x42; 32])),
        )
        .unwrap();

        coin.register_pending_message("first-message").unwrap();
        assert!(coin.register_pending_message("second-message").is_err());
        coin.remove_pending_message("first-message");
        assert!(coin.register_pending_message("second-message").is_ok());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn restores_an_unresolved_message_before_the_next_broadcast() {
        let path = std::env::temp_dir().join(format!("kdf-ton-pending-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let coin = TonCoin::new(
            config(),
            request(),
            PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from([0x42; 32])),
        )
        .unwrap();
        *coin.0.pending_messages_path.lock() = Some(path.clone());
        coin.register_pending_message("persisted-message").unwrap();
        block_on(coin.persist_pending_messages()).unwrap();

        let restored = TonCoin::new(
            config(),
            request(),
            PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from([0x42; 32])),
        )
        .unwrap();
        *restored.0.pending_messages_path.lock() = Some(path.clone());
        *restored.0.pending_messages.lock() = block_on(load_persisted_pending_messages(&path))
            .unwrap()
            .into_iter()
            .collect();
        assert!(restored.register_pending_message("replacement-message").is_err());
        std::fs::remove_file(path).unwrap();
    }
}
