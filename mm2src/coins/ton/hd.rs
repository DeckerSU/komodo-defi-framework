use crypto::GlobalHDAccountArc;
use derive_more::Display;
use ed25519_dalek_bip32::DerivationPath as Ed25519DerivationPath;
use std::error::Error;
use std::str::FromStr;

/// TEP-3 multichain path for the first TON Ed25519 account.
///
/// This derives from the KDF BIP39 seed through SLIP-10. It is deliberately
/// distinct from TON's native mnemonic derivation, so a KDF HD wallet can use
/// one BIP39 phrase for every supported coin.
pub const TON_MULTICHAIN_DERIVATION_PATH: &str = "m/44'/607'/0'";

/// Derives the first TON Ed25519 signing seed from KDF's global BIP39 account.
pub fn derive_ton_hd_seed(global_hd: &GlobalHDAccountArc) -> Result<[u8; 32], TonHdDerivationError> {
    let path = Ed25519DerivationPath::from_str(TON_MULTICHAIN_DERIVATION_PATH)
        .map_err(|error| TonHdDerivationError::InvalidDerivationPath(error.to_string()))?;
    let extended_key = global_hd
        .derive_ed25519_signing_key(&path)
        .map_err(|error| TonHdDerivationError::KeyDerivation(error.into_inner().to_string()))?;
    Ok(extended_key.signing_key.to_bytes())
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonHdDerivationError {
    #[display(fmt = "Invalid TON HD derivation path: {_0}")]
    InvalidDerivationPath(String),
    #[display(fmt = "Unable to derive TON HD signing key: {_0}")]
    KeyDerivation(String),
}

impl Error for TonHdDerivationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ton::{TonAddressFormat, TonNetwork, TonWalletParams};
    use ed25519_dalek_bip32::ExtendedSigningKey;

    const BIP39_TEST_SEED: [u8; 64] = [
        0x5e, 0xb0, 0x0b, 0xbd, 0xdc, 0xf0, 0x69, 0x08, 0x48, 0x89, 0xa8, 0xab, 0x91, 0x55, 0x56, 0x81, 0x65, 0xf5,
        0xc4, 0x53, 0xcc, 0xb8, 0x5e, 0x70, 0x81, 0x1a, 0xae, 0xd6, 0xf6, 0xda, 0x5f, 0xc1, 0x9a, 0x5a, 0xc4, 0x0b,
        0x38, 0x9c, 0xd3, 0x70, 0xd0, 0x86, 0x20, 0x6d, 0xec, 0x8a, 0xa6, 0xc4, 0x3d, 0xae, 0xa6, 0x69, 0x0f, 0x20,
        0xad, 0x3d, 0x8d, 0x48, 0xb2, 0xd2, 0xce, 0x9e, 0x38, 0xe4,
    ];
    const TON_HD_SEED: [u8; 32] = [
        0xb4, 0x77, 0xef, 0x5e, 0xd1, 0x7f, 0xb8, 0xa2, 0xb8, 0xfa, 0xdd, 0xd7, 0xa9, 0x83, 0x5a, 0x22, 0x72, 0x43,
        0xa8, 0x2c, 0x70, 0xb1, 0x90, 0xc7, 0xaf, 0x48, 0x96, 0x15, 0x5a, 0xa7, 0xdf, 0x9f,
    ];
    const TON_HD_ADDRESS: &str = "UQBHyu-oZVDHRYQ1-rKlGqpHy5yAqanPBirEQNMNOmfHLtaT";

    #[test]
    fn multichain_bip39_vector_derives_the_first_ton_w5_address() {
        let master = ExtendedSigningKey::from_seed(&BIP39_TEST_SEED).unwrap();
        let path = Ed25519DerivationPath::from_str(TON_MULTICHAIN_DERIVATION_PATH).unwrap();
        let seed = master.derive(&path).unwrap().signing_key.to_bytes();

        assert_eq!(seed, TON_HD_SEED);
        let address = TonWalletParams::MAINNET_DEFAULT.address_from_seed(&seed).unwrap();
        assert_eq!(
            address.format(
                TonAddressFormat::Friendly {
                    bounceable: false,
                    urlsafe: true,
                },
                TonNetwork::Mainnet,
            ),
            TON_HD_ADDRESS,
        );
    }
}
