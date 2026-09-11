use super::{TonAccountState, TonWalletInformation};
use derive_more::Display;
use std::error::Error;

/// Sender state required to build a W5 external message.
///
/// The caller must still verify that an active account contains compatible W5R1
/// code before relying on this state. This type only prevents an activation or
/// withdrawal flow from silently treating a frozen/unknown account as deployable
/// or substituting a missing active-account sequence number with zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TonTransferState {
    /// An account without deployed state. The first W5 message includes StateInit.
    Deployable { sequence_number: u32 },
    /// An existing account. Its next message must omit StateInit.
    Active { sequence_number: u32 },
}

impl TonTransferState {
    pub const fn sequence_number(self) -> u32 {
        match self {
            TonTransferState::Deployable { sequence_number } | TonTransferState::Active { sequence_number } => {
                sequence_number
            },
        }
    }

    pub const fn deploy_wallet(self) -> bool {
        matches!(self, TonTransferState::Deployable { .. })
    }
}

impl TonWalletInformation {
    /// Converts provider account state into safe W5 transfer inputs.
    pub fn transfer_state(&self) -> Result<TonTransferState, TonAccountStateError> {
        match self.account_state {
            TonAccountState::Uninitialized => Ok(TonTransferState::Deployable { sequence_number: 0 }),
            TonAccountState::Active => self
                .sequence_number
                .map(|sequence_number| TonTransferState::Active { sequence_number })
                .ok_or(TonAccountStateError::MissingActiveSequenceNumber),
            TonAccountState::Frozen => Err(TonAccountStateError::Frozen),
            TonAccountState::Unknown => Err(TonAccountStateError::Unknown),
        }
    }
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonAccountStateError {
    #[display(fmt = "Active TON account has no sequence number")]
    MissingActiveSequenceNumber,
    #[display(fmt = "TON account is frozen")]
    Frozen,
    #[display(fmt = "TON account state is unknown")]
    Unknown,
}

impl Error for TonAccountStateError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ton::TonAmount;

    fn wallet_information(account_state: TonAccountState, sequence_number: Option<u32>) -> TonWalletInformation {
        TonWalletInformation {
            balance: TonAmount::ZERO,
            account_state,
            sequence_number,
            wallet_type: None,
        }
    }

    #[test]
    fn only_an_uninitialized_account_is_deployable() {
        let state = wallet_information(TonAccountState::Uninitialized, None)
            .transfer_state()
            .unwrap();
        assert_eq!(state, TonTransferState::Deployable { sequence_number: 0 });
        assert!(state.deploy_wallet());
    }

    #[test]
    fn active_account_requires_its_provider_sequence_number() {
        assert_eq!(
            wallet_information(TonAccountState::Active, None).transfer_state(),
            Err(TonAccountStateError::MissingActiveSequenceNumber),
        );
        let state = wallet_information(TonAccountState::Active, Some(7))
            .transfer_state()
            .unwrap();
        assert_eq!(state, TonTransferState::Active { sequence_number: 7 });
        assert!(!state.deploy_wallet());
    }

    #[test]
    fn frozen_and_unknown_accounts_are_not_deployable() {
        assert_eq!(
            wallet_information(TonAccountState::Frozen, None).transfer_state(),
            Err(TonAccountStateError::Frozen),
        );
        assert_eq!(
            wallet_information(TonAccountState::Unknown, None).transfer_state(),
            Err(TonAccountStateError::Unknown),
        );
    }
}
