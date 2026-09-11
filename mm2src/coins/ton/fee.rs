use super::{TonFeeComponent, TonFeeEstimate, TON_DECIMALS};
use crate::utxo::utxo_common::big_decimal_from_sat_unsigned;
use mm2_number::BigDecimal;
use serde::{Deserialize, Serialize};

/// One component of TON Center's simulated transaction fees, expressed in
/// GRAM rather than nanoGRAM.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TonFeeComponentDetails {
    pub in_forward_fee: BigDecimal,
    pub storage_fee: BigDecimal,
    pub gas_fee: BigDecimal,
    pub forward_fee: BigDecimal,
}

impl From<TonFeeComponent> for TonFeeComponentDetails {
    fn from(component: TonFeeComponent) -> Self {
        TonFeeComponentDetails {
            in_forward_fee: big_decimal_from_sat_unsigned(component.in_forward_fee, TON_DECIMALS),
            storage_fee: big_decimal_from_sat_unsigned(component.storage_fee, TON_DECIMALS),
            gas_fee: big_decimal_from_sat_unsigned(component.gas_fee, TON_DECIMALS),
            forward_fee: big_decimal_from_sat_unsigned(component.forward_fee, TON_DECIMALS),
        }
    }
}

/// Fee information returned with a signed GRAM withdrawal.
///
/// `total_fee` is the simulated debit from the source wallet. Destination
/// components are retained as provider diagnostics and are not added to the
/// sender's balance requirement.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TonTxFeeDetails {
    pub coin: String,
    pub source: TonFeeComponentDetails,
    pub destinations: Vec<TonFeeComponentDetails>,
    pub total_fee: BigDecimal,
}

impl TonTxFeeDetails {
    pub fn from_estimate(coin: String, estimate: &TonFeeEstimate) -> Result<Self, super::TonRpcError> {
        let total_fee = big_decimal_from_sat_unsigned(estimate.source_total()?, TON_DECIMALS);
        Ok(TonTxFeeDetails {
            coin,
            source: estimate.source.into(),
            destinations: estimate.destinations.iter().copied().map(Into::into).collect(),
            total_fee,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_fee_excludes_destination_diagnostics() {
        let estimate = TonFeeEstimate {
            source: TonFeeComponent {
                in_forward_fee: 1,
                storage_fee: 2,
                gas_fee: 3,
                forward_fee: 4,
            },
            destinations: vec![TonFeeComponent {
                in_forward_fee: 5,
                storage_fee: 6,
                gas_fee: 7,
                forward_fee: 8,
            }],
        };

        let details = TonTxFeeDetails::from_estimate("GRAM".to_owned(), &estimate).unwrap();
        assert_eq!(details.total_fee, big_decimal_from_sat_unsigned(10, TON_DECIMALS));
        assert_eq!(details.destinations.len(), 1);
    }
}
