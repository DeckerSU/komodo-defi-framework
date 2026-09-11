use super::{TonAddress, TonNetwork};
use derive_more::Display;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use tonlib_core::wallet::{mnemonic::KeyPair, ton_wallet::TonWallet, wallet_version::WalletVersion};
use zeroize::Zeroize;

const W5_CONTEXT_CLIENT: u32 = 1 << 31;
const W5_CONTEXT_WORKCHAIN_SHIFT: u32 = 23;
const W5_SUBWALLET_MAX: u16 = (1 << 15) - 1;

/// W5R1 subwallet counter. Only counter zero is enabled by the initial KDF integration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TonSubwalletId(u16);

impl TonSubwalletId {
    pub const DEFAULT: TonSubwalletId = TonSubwalletId(0);

    pub fn new(value: u16) -> Result<Self, TonWalletError> {
        if value > W5_SUBWALLET_MAX {
            return Err(TonWalletError::InvalidSubwalletId(value));
        }
        Ok(TonSubwalletId(value))
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

impl Serialize for TonSubwalletId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for TonSubwalletId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u16::deserialize(deserializer)?;
        TonSubwalletId::new(value).map_err(serde::de::Error::custom)
    }
}

/// Parameters that take part in W5R1 address derivation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TonWalletParams {
    pub network: TonNetwork,
    pub workchain: i8,
    pub subwallet_id: TonSubwalletId,
}

impl TonWalletParams {
    pub const MAINNET_DEFAULT: TonWalletParams = TonWalletParams {
        network: TonNetwork::Mainnet,
        workchain: 0,
        subwallet_id: TonSubwalletId::DEFAULT,
    };

    pub const TESTNET_DEFAULT: TonWalletParams = TonWalletParams {
        network: TonNetwork::Testnet,
        workchain: 0,
        subwallet_id: TonSubwalletId::DEFAULT,
    };

    /// Computes the W5R1 client wallet ID described by the TON wallet standard.
    pub const fn wallet_id(self) -> i32 {
        let context = W5_CONTEXT_CLIENT
            | ((self.workchain as u8 as u32) << W5_CONTEXT_WORKCHAIN_SHIFT)
            | self.subwallet_id.value() as u32;
        (self.network.global_id() as u32 ^ context) as i32
    }

    /// Builds a W5R1 wallet from a 32-byte Ed25519 signing seed.
    ///
    /// Iguana mode supplies this seed from its existing 32-byte private key.
    pub fn wallet_from_seed(self, seed: &[u8; 32]) -> Result<TonWallet, TonWalletError> {
        let mut keypair = nacl::sign::generate_keypair(seed);
        let key_pair = KeyPair {
            public_key: keypair.pkey.to_vec(),
            secret_key: keypair.skey.to_vec(),
        };
        keypair.skey.zeroize();
        TonWallet::new_with_params(WalletVersion::V5R1, key_pair, self.workchain as i32, self.wallet_id())
            .map_err(|_| TonWalletError::WalletConstruction)
    }

    pub fn address_from_seed(self, seed: &[u8; 32]) -> Result<TonAddress, TonWalletError> {
        let wallet = self.wallet_from_seed(seed)?;
        Ok(TonAddress::from_inner(wallet.address))
    }
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonWalletError {
    #[display(fmt = "Invalid TON W5 subwallet ID: {_0}")]
    InvalidSubwalletId(u16),
    #[display(fmt = "Unable to construct TON W5R1 wallet")]
    WalletConstruction,
}

impl Error for TonWalletError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ton::TonAddressFormat;
    use crypto::privkey::key_pair_from_seed;

    const IGUANA_PASSPHRASE_VECTOR: &str = "kdf-ton-iguana-vector";
    const IGUANA_W5_MAINNET_VECTOR: &str = "UQBZfhh5F-CFw-1L978b7jrJ0c3FUlssE8jk2ueScxRHleke";
    #[test]
    fn seed_wallets_and_subwallets_are_deterministic() {
        let seed = [7; 32];
        let first = TonWalletParams::MAINNET_DEFAULT.address_from_seed(&seed).unwrap();
        let repeated = TonWalletParams::MAINNET_DEFAULT.address_from_seed(&seed).unwrap();
        let second = TonWalletParams {
            subwallet_id: TonSubwalletId::new(1).unwrap(),
            ..TonWalletParams::MAINNET_DEFAULT
        }
        .address_from_seed(&seed)
        .unwrap();

        assert_eq!(first, repeated);
        assert_ne!(first, second);
    }

    #[test]
    fn iguana_key_construction_derives_its_own_w5_address() {
        let key_pair = key_pair_from_seed(IGUANA_PASSPHRASE_VECTOR).unwrap();
        let address = TonWalletParams::MAINNET_DEFAULT
            .address_from_seed(&key_pair.private_bytes())
            .unwrap();

        assert_eq!(
            address.format(
                TonAddressFormat::Friendly {
                    bounceable: false,
                    urlsafe: true,
                },
                TonNetwork::Mainnet,
            ),
            IGUANA_W5_MAINNET_VECTOR,
        );
    }

    #[test]
    fn rejects_subwallet_ids_outside_the_w5_range() {
        assert_eq!(
            TonSubwalletId::new(W5_SUBWALLET_MAX + 1),
            Err(TonWalletError::InvalidSubwalletId(W5_SUBWALLET_MAX + 1)),
        );
    }
}
