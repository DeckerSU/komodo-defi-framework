use super::{
    TonActivationError, TonActivationRequest, TonAddress, TonAddressFormat, TonAmount, TonCoinConfig, TonTxFeeDetails,
    TonWalletContext, TonWalletInformation, TON_DECIMALS,
};
use crate::coin_errors::{AddressFromPubkeyError, MyAddressError};
use crate::coin_errors::{ValidatePaymentError, ValidatePaymentResult};
use crate::hd_wallet::HDAddressSelector;
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
use std::collections::HashSet;
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
}

const TON_SWAP_UNSUPPORTED: &str = "TON atomic swaps are not supported; GRAM is wallet-only";
const DEFAULT_TRANSFER_EXPIRATION_SECONDS: u64 = 60;
const MAX_TRANSFER_EXPIRATION_SECONDS: u64 = 3_600;
const MAX_PENDING_MESSAGES: usize = 32;

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
        if input.confirmations != 1 || input.requires_nota {
            return Box::new(futures01::future::err(
                "TON currently supports one account-inclusion confirmation only".to_owned(),
            ));
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
                        .account_transactions(MAX_PENDING_MESSAGES as u8)
                        .await
                        .map_err(|error| error.to_string())?;
                    if transactions.iter().any(|transaction| {
                        transaction
                            .inbound_message_hash
                            .as_deref()
                            .map_or(false, |hash| ton_hashes_equal(&message_hash, hash))
                    }) {
                        coin.remove_pending_message(&message_hash);
                        return Ok(());
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
    fn process_history_loop(&self, _ctx: mm2_core::mm_ctx::MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(futures01::future::ok(()))
    }
    fn history_sync_status(&self) -> HistorySyncState {
        HistorySyncState::NotEnabled
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
        Ok(TonCoin(Arc::new(TonCoinFields {
            wallet,
            abortable_system: AbortableQueue::default(),
            required_confirmations: AtomicU64::new(required_confirmations),
            pending_messages: Mutex::new(HashSet::new()),
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

    fn register_pending_message(&self, message_hash: &str) -> Result<(), String> {
        let mut pending = self.0.pending_messages.lock();
        if pending.contains(message_hash) {
            return Err("TON external message is already pending reconciliation".to_owned());
        }
        if pending.len() >= MAX_PENDING_MESSAGES {
            return Err("TON pending-message limit reached; reconcile existing broadcasts first".to_owned());
        }
        pending.insert(message_hash.to_owned());
        Ok(())
    }

    fn remove_pending_message(&self, message_hash: &str) {
        self.0.pending_messages.lock().remove(message_hash);
    }

    /// Removes locally pending external messages once their exact inbound
    /// message hash appears in the account's newest transactions. This is
    /// inclusion reconciliation only: it does not claim recipient execution
    /// or masterchain confirmation.
    pub async fn reconcile_pending_messages(&self) -> Result<usize, TonActivationError> {
        let transactions = self.0.wallet.account_transactions(MAX_PENDING_MESSAGES as u8).await?;
        let observed: Vec<_> = transactions
            .iter()
            .filter_map(|transaction| transaction.inbound_message_hash.as_deref())
            .collect();
        let mut pending = self.0.pending_messages.lock();
        let before = pending.len();
        pending.retain(|pending_hash| !observed.iter().any(|hash| ton_hashes_equal(pending_hash, hash)));
        Ok(before - pending.len())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ton::{build_signed_transfer, TonNetwork, TonTransferRequest, TonWalletParams},
        IguanaPrivKey, PrivKeyBuildPolicy,
    };
    use serde_json::json;

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
}
