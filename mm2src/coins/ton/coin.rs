use super::{
    TonActivationError, TonActivationRequest, TonAddress, TonCoinConfig, TonWalletContext, TonWalletInformation,
};
use crate::PrivKeyBuildPolicy;
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
