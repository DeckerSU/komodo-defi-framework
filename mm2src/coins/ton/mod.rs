//! Primitives shared by the native TON wallet implementation.
//!
//! TON addresses identify contract state, not an Ed25519 public key. The W5R1
//! wallet helpers in this module therefore keep the key derivation and wallet
//! parameters explicit.

mod address;
mod wallet;

pub use address::{TonAddress, TonAddressError, TonAddressFormat, TonNetwork};
pub use wallet::{TonSubwalletId, TonWalletError, TonWalletParams};

pub const TON_DECIMALS: u8 = 9;
