use super::{derive_ton_hd_seed, TonAddress, TonWalletError, TonWalletParams};
use crate::PrivKeyBuildPolicy;
use derive_more::Display;
use std::error::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// KDF-owned Ed25519 signing material for a TON wallet.
///
/// This is deliberately not serializable or cloneable. TON library wallet objects
/// are created only for the operation that needs them, while this type keeps the
/// long-lived 32-byte seed zeroized on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct TonSigningSeed([u8; 32]);

impl TonSigningSeed {
    /// Selects the TON seed from KDF's established activation key policy.
    ///
    /// Iguana uses its existing 32-byte secp256k1 secret bytes directly as an
    /// Ed25519 seed. HD derives a distinct Ed25519 seed from the primary KDF
    /// BIP39 wallet at the fixed TEP-3 multichain path.
    pub fn from_priv_key_policy(policy: PrivKeyBuildPolicy) -> Result<Self, TonKeyPolicyError> {
        let seed = match policy {
            PrivKeyBuildPolicy::IguanaPrivKey(iguana) => {
                let mut seed = [0; 32];
                seed.copy_from_slice(iguana.as_slice());
                seed
            },
            PrivKeyBuildPolicy::GlobalHDAccount(global_hd) => {
                derive_ton_hd_seed(&global_hd).map_err(|_| TonKeyPolicyError::HdDerivation)?
            },
            PrivKeyBuildPolicy::Trezor => return Err(TonKeyPolicyError::HardwareWalletNotSupported),
            PrivKeyBuildPolicy::WalletConnect { .. } => return Err(TonKeyPolicyError::WalletConnectNotSupported),
        };
        Ok(TonSigningSeed(seed))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn address(&self, wallet_params: TonWalletParams) -> Result<TonAddress, TonWalletError> {
        wallet_params.address_from_seed(self.as_bytes())
    }

    pub(crate) fn wallet(
        &self,
        wallet_params: TonWalletParams,
    ) -> Result<tonlib_core::wallet::ton_wallet::TonWallet, TonWalletError> {
        wallet_params.wallet_from_seed(self.as_bytes())
    }
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonKeyPolicyError {
    #[display(fmt = "Unable to derive the TON HD signing key")]
    HdDerivation,
    #[display(fmt = "TON hardware-wallet activation is not supported")]
    HardwareWalletNotSupported,
    #[display(fmt = "TON WalletConnect activation is not supported")]
    WalletConnectNotSupported,
}

impl Error for TonKeyPolicyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ton::{TonAddressFormat, TonNetwork},
        IguanaPrivKey,
    };

    #[test]
    fn iguana_uses_the_existing_private_key_bytes_as_the_ed25519_seed() {
        let private_key = [0x42; 32];
        let signing_seed =
            TonSigningSeed::from_priv_key_policy(PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from(private_key)))
                .unwrap();

        assert_eq!(signing_seed.as_bytes(), &private_key);
        assert_eq!(
            signing_seed.address(TonWalletParams::MAINNET_DEFAULT).unwrap().format(
                TonAddressFormat::Friendly {
                    bounceable: false,
                    urlsafe: true,
                },
                TonNetwork::Mainnet,
            ),
            "UQA_O1iT-mrBM2FBjVKUiM9O6Qv--yzmD9F8bIXS3aq6jrad",
        );
    }

    #[test]
    fn rejects_key_policies_without_a_supported_ton_derivation() {
        assert!(matches!(
            TonSigningSeed::from_priv_key_policy(PrivKeyBuildPolicy::Trezor),
            Err(TonKeyPolicyError::HardwareWalletNotSupported)
        ));
    }
}
