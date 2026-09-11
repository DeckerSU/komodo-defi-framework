//! Primitives shared by the native TON wallet implementation.
//!
//! TON addresses identify contract state, not an Ed25519 public key. The W5R1
//! wallet helpers in this module therefore keep the key derivation and wallet
//! parameters explicit.

mod address;
mod amount;
mod client;
mod hd;
mod key;
mod protocol;
mod transaction;
mod wallet;

pub use address::{TonAddress, TonAddressError, TonAddressFormat, TonNetwork};
pub use amount::{TonAmount, TonAmountError};
pub use client::{TonAccountState, TonRpcClient, TonRpcError, TonWalletInformation};
pub use hd::{derive_ton_hd_seed, TonHdDerivationError, TON_MULTICHAIN_DERIVATION_PATH};
pub use key::{TonKeyPolicyError, TonSigningSeed};
pub use protocol::{TonProtocolInfo, TonWalletVersion};
pub use transaction::{build_signed_transfer, TonSignedTransfer, TonTransferError, TonTransferRequest};
pub use wallet::{TonSubwalletId, TonWalletError, TonWalletParams};

pub const TON_DECIMALS: u8 = 9;
