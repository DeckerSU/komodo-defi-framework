use super::{
    TonActivationError, TonActivationRequest, TonAddress, TonAddressFormat, TonCoinConfig, TonWalletContext,
    TonWalletInformation, TON_DECIMALS,
};
use crate::coin_errors::{AddressFromPubkeyError, MyAddressError};
use crate::hd_wallet::HDAddressSelector;
use crate::utxo::utxo_common::big_decimal_from_sat_unsigned;
use crate::{
    BalanceError, BalanceFut, CoinBalance, ConfirmPaymentInput, MarketCoinOps, PrivKeyBuildPolicy, SignatureError,
    SignatureResult, TransactionErr, TransactionResult, TxMarshalingErr, UnexpectedDerivationMethod, VerificationError,
    VerificationResult, WaitForHTLCTxSpendArgs,
};
use async_trait::async_trait;
use futures::compat::Future01CompatExt;
use futures::FutureExt;
use futures01::Future;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use rpc::v1::types::H264 as H264Json;
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

impl TonCoin {
    pub fn new(
        config: TonCoinConfig,
        request: TonActivationRequest,
        key_policy: PrivKeyBuildPolicy,
    ) -> Result<Self, TonActivationError> {
        let wallet = TonWalletContext::new(config, request, key_policy)?;
        Ok(TonCoin(Arc::new(TonCoinFields { wallet })))
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
