use super::{
    TonActivationError, TonActivationRequest, TonAddress, TonAddressFormat, TonCoinConfig, TonWalletContext,
    TonWalletInformation, TON_DECIMALS,
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
    TradePreimageResult, TradePreimageValue, TransactionErr, TransactionResult, TxMarshalingErr,
    UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs, ValidateOtherPubKeyErr, ValidatePaymentInput,
    VerificationError, VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WeakSpawner, WithdrawError, WithdrawFut,
    WithdrawRequest,
};
use async_trait::async_trait;
use common::executor::{abortable_queue::AbortableQueue, AbortableSystem, AbortedError};
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use keys::KeyPair;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use rpc::v1::types::Bytes as BytesJson;
use rpc::v1::types::H264 as H264Json;
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
}

const TON_SWAP_UNSUPPORTED: &str = "TON atomic swaps are not supported; GRAM is wallet-only";

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

    fn send_raw_tx(&self, _tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        Box::new(futures01::future::err(
            "TON raw BOC broadcast validation is not implemented".to_owned(),
        ))
    }

    fn send_raw_tx_bytes(&self, _tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        Box::new(futures01::future::err(
            "TON raw BOC broadcast validation is not implemented".to_owned(),
        ))
    }

    fn wait_for_confirmations(&self, _input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        Box::new(futures01::future::err(
            "TON confirmation tracking is not implemented".to_owned(),
        ))
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
        Box::new(futures01::future::err(
            "TON masterchain lookup is not implemented".to_owned(),
        ))
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
    fn withdraw(&self, _req: WithdrawRequest) -> WithdrawFut {
        Box::new(futures01::future::err(MmError::new(WithdrawError::UnsupportedError(
            "TON withdrawal requires fee estimation and is not implemented".to_owned(),
        ))))
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
        })))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IguanaPrivKey, PrivKeyBuildPolicy};
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
}
