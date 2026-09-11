use super::{TonAddress, TonAmount, TonAmountError, TonNetwork};
use derive_more::Display;
use num_bigint::BigUint;
use std::error::Error;
use tonlib_core::{
    cell::CellBuilder,
    message::{CommonMsgInfo, InternalMessage, TonMessage, TransferMessage},
    tlb_types::tlb::TLB,
    wallet::ton_wallet::TonWallet,
    TonAddress as InnerTonAddress,
};

/// The data KDF needs to create one W5R1 native transfer.
#[derive(Clone, Debug)]
pub struct TonTransferRequest {
    pub recipient: TonAddress,
    pub amount: TonAmount,
    pub bounceable: bool,
    pub sequence_number: u32,
    pub expire_at: u32,
    pub deploy_wallet: bool,
}

/// A signed external TON message ready for base64 encoding and broadcast.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonSignedTransfer {
    pub boc: Vec<u8>,
    /// The external message cell hash. It is not an eventual account transaction hash.
    pub message_hash: String,
}

pub fn build_signed_transfer(
    wallet: &TonWallet,
    network: TonNetwork,
    request: &TonTransferRequest,
) -> Result<TonSignedTransfer, TonTransferError> {
    request
        .recipient
        .ensure_network(network)
        .map_err(|_| TonTransferError::RecipientNetwork)?;
    request.amount.require_non_zero()?;
    if let Some(actual) = request.recipient.is_bounceable() {
        if actual != request.bounceable {
            return Err(TonTransferError::RecipientBounceModeMismatch);
        }
    }

    let internal = build_internal_transfer(request)?;
    let external = wallet
        .create_external_msg(
            request.expire_at,
            request.sequence_number,
            request.deploy_wallet,
            &[internal.to_arc()],
        )
        .map_err(|_| TonTransferError::MessageConstruction)?;
    let message_hash = external.cell_hash().to_hex();
    let boc = external.to_boc(true).map_err(|_| TonTransferError::BocSerialization)?;

    Ok(TonSignedTransfer { boc, message_hash })
}

fn build_internal_transfer(request: &TonTransferRequest) -> Result<tonlib_core::cell::Cell, TonTransferError> {
    let body = CellBuilder::new()
        .build()
        .map_err(|_| TonTransferError::MessageConstruction)?;
    TransferMessage::new(
        CommonMsgInfo::InternalMessage(InternalMessage {
            ihr_disabled: true,
            bounce: request.bounceable,
            bounced: false,
            src: InnerTonAddress::NULL,
            dest: request.recipient.inner().clone(),
            value: BigUint::from(request.amount.as_nano()),
            ihr_fee: BigUint::default(),
            fwd_fee: BigUint::default(),
            created_lt: 0,
            created_at: 0,
        }),
        body.to_arc(),
    )
    .build()
    .map_err(|_| TonTransferError::MessageConstruction)
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonTransferError {
    #[display(fmt = "TON recipient address belongs to a different network")]
    RecipientNetwork,
    #[display(fmt = "TON transfer bounce mode conflicts with the recipient address")]
    RecipientBounceModeMismatch,
    #[display(fmt = "Unable to construct TON transfer message")]
    MessageConstruction,
    #[display(fmt = "Unable to serialize TON transfer BOC")]
    BocSerialization,
    #[display(fmt = "Invalid TON transfer amount")]
    Amount,
}

impl Error for TonTransferError {}

impl From<TonAmountError> for TonTransferError {
    fn from(_: TonAmountError) -> Self {
        TonTransferError::Amount
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ton::{TonAddressFormat, TonWalletParams};
    use tonlib_core::tlb_types::{
        block::message::{CommonMsgInfo as BlockCommonMsgInfo, Message},
        tlb::TLB,
    };

    const TON_MNEMONIC_VECTOR: &str =
        "section garden tomato dinner season dice renew length useful spin trade intact use universe what post spike keen mandate behind concert egg doll rug";
    const DESTINATION: &str = "UQBYGTsWwxh00p3Fq_EdwzQ2uRzuptfxP5crEOsfRT6zDOS4";

    fn request() -> TonTransferRequest {
        TonTransferRequest {
            recipient: TonAddress::parse(DESTINATION).unwrap(),
            amount: "0.001".parse().unwrap(),
            bounceable: false,
            sequence_number: 7,
            expire_at: 1_700_000_000,
            deploy_wallet: true,
        }
    }

    #[test]
    fn builds_a_serializable_deploy_transfer() {
        let wallet = TonWalletParams::MAINNET_DEFAULT
            .wallet_from_mnemonic(TON_MNEMONIC_VECTOR)
            .unwrap();
        let signed = build_signed_transfer(&wallet, TonNetwork::Mainnet, &request()).unwrap();
        let external = Message::from_boc(&signed.boc).unwrap();

        assert!(!signed.boc.is_empty());
        assert_eq!(signed.message_hash.len(), 64);
        assert!(external.init.is_some());
        match external.info {
            BlockCommonMsgInfo::ExtIn(info) => assert_eq!(info.dest, wallet.address.to_msg_address_int()),
            _ => panic!("expected an external incoming message"),
        }
    }

    #[test]
    fn internal_transfer_preserves_recipient_amount_and_bounce_mode() {
        let request = request();
        let internal = build_internal_transfer(&request).unwrap();
        let internal = TransferMessage::parse(&internal).unwrap();

        match internal.common_msg_info {
            CommonMsgInfo::InternalMessage(info) => {
                assert_eq!(info.dest, *request.recipient.inner());
                assert_eq!(info.value, BigUint::from(request.amount.as_nano()));
                assert!(!info.bounce);
            },
            _ => panic!("expected an internal message"),
        }
    }

    #[test]
    fn rejects_network_and_bounce_mode_conflicts() {
        let wallet = TonWalletParams::MAINNET_DEFAULT
            .wallet_from_mnemonic(TON_MNEMONIC_VECTOR)
            .unwrap();
        let mut request = request();

        request.bounceable = true;
        assert_eq!(
            build_signed_transfer(&wallet, TonNetwork::Mainnet, &request),
            Err(TonTransferError::RecipientBounceModeMismatch),
        );

        request.bounceable = false;
        request.recipient = TonAddress::parse(&request.recipient.format(
            TonAddressFormat::Friendly {
                bounceable: false,
                urlsafe: true,
            },
            TonNetwork::Testnet,
        ))
        .unwrap();
        assert_eq!(
            build_signed_transfer(&wallet, TonNetwork::Mainnet, &request),
            Err(TonTransferError::RecipientNetwork),
        );
    }
}
