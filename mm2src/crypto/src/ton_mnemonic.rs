use bip39::Language;
use derive_more::Display;
use hmac::{Hmac, Mac};
use nacl::sign::generate_keypair;
use pbkdf2::pbkdf2_hmac;
use sha2::Sha512;
use std::convert::TryInto;
use std::error::Error;
use std::ops::Deref;
use std::sync::Arc;
use zeroize::Zeroize;

const TON_PBKDF_ITERATIONS: u32 = 100_000;
const TON_PASSWORDLESS_VALIDATION_ITERATIONS: u32 = TON_PBKDF_ITERATIONS / 256;

/// An Ed25519 signing seed derived with the native TON mnemonic algorithm.
///
/// This is deliberately distinct from `GlobalHDAccountCtx`: TON mnemonic validation
/// and derivation are not BIP39/SLIP-10. The seed is held only by this zeroizing type.
pub struct TonMnemonicKey {
    ed25519_seed: [u8; 32],
}

impl Drop for TonMnemonicKey {
    fn drop(&mut self) {
        self.ed25519_seed.zeroize();
    }
}

impl TonMnemonicKey {
    pub fn from_mnemonic(mnemonic: &str) -> Result<Self, TonMnemonicError> {
        let mut normalized = normalize_ton_mnemonic(mnemonic)?;
        let mut validation = match derive_ton_seed(
            normalized.as_bytes(),
            b"TON seed version",
            TON_PASSWORDLESS_VALIDATION_ITERATIONS,
        ) {
            Ok(seed) => seed,
            Err(error) => {
                normalized.zeroize();
                return Err(error);
            },
        };
        let is_passwordless_ton_mnemonic = validation[0] == 0;
        validation.zeroize();
        if !is_passwordless_ton_mnemonic {
            normalized.zeroize();
            return Err(TonMnemonicError::InvalidMnemonic);
        }

        let mut key_material = match derive_ton_seed(normalized.as_bytes(), b"TON default seed", TON_PBKDF_ITERATIONS) {
            Ok(seed) => seed,
            Err(error) => {
                normalized.zeroize();
                return Err(error);
            },
        };
        normalized.zeroize();

        let mut key_pair = generate_keypair(&key_material[..32]);
        key_material.zeroize();
        let ed25519_seed = key_pair.skey[..32]
            .try_into()
            .map_err(|_| TonMnemonicError::KeyDerivation);
        key_pair.skey.zeroize();
        key_pair.pkey.zeroize();

        ed25519_seed.map(|ed25519_seed| TonMnemonicKey { ed25519_seed })
    }

    pub fn ed25519_seed(&self) -> &[u8; 32] {
        &self.ed25519_seed
    }

    pub fn into_arc(self) -> TonMnemonicKeyArc {
        TonMnemonicKeyArc(Arc::new(self))
    }
}

#[derive(Clone)]
pub struct TonMnemonicKeyArc(Arc<TonMnemonicKey>);

impl Deref for TonMnemonicKeyArc {
    type Target = TonMnemonicKey;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonMnemonicError {
    #[display(fmt = "Invalid TON mnemonic")]
    InvalidMnemonic,
    #[display(fmt = "Unable to derive TON mnemonic key")]
    KeyDerivation,
}

impl Error for TonMnemonicError {}

fn normalize_ton_mnemonic(mnemonic: &str) -> Result<String, TonMnemonicError> {
    let mut normalized = String::with_capacity(mnemonic.len());
    let mut word_count = 0;

    for word in mnemonic.split_whitespace() {
        let word = word.to_lowercase();
        if Language::English.find_word(&word).is_none() {
            normalized.zeroize();
            return Err(TonMnemonicError::InvalidMnemonic);
        }
        if word_count != 0 {
            normalized.push(' ');
        }
        normalized.push_str(&word);
        word_count += 1;
    }

    if word_count != 24 {
        normalized.zeroize();
        return Err(TonMnemonicError::InvalidMnemonic);
    }
    Ok(normalized)
}

fn derive_ton_seed(words: &[u8], salt: &[u8], iterations: u32) -> Result<[u8; 64], TonMnemonicError> {
    let mac = Hmac::<Sha512>::new_from_slice(words).map_err(|_| TonMnemonicError::KeyDerivation)?;
    let mut entropy = mac.finalize().into_bytes();
    let mut seed = [0; 64];
    pbkdf2_hmac::<Sha512>(&entropy, salt, iterations, &mut seed);
    entropy.zeroize();
    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TON_MNEMONIC_VECTOR: &str =
        "section garden tomato dinner season dice renew length useful spin trade intact use universe what post spike keen mandate behind concert egg doll rug";

    #[test]
    fn accepts_a_native_ton_mnemonic() {
        let key = TonMnemonicKey::from_mnemonic(TON_MNEMONIC_VECTOR).unwrap();
        assert_ne!(key.ed25519_seed(), &[0; 32]);
    }

    #[test]
    fn rejects_an_invalid_native_ton_mnemonic() {
        assert!(matches!(
            TonMnemonicKey::from_mnemonic("not a valid TON mnemonic"),
            Err(TonMnemonicError::InvalidMnemonic)
        ));
    }
}
