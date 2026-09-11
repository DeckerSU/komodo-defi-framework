use crate::ton::{TonNetwork, TonSubwalletId, TonWalletParams};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The only wallet contract version supported by the initial GRAM integration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TonWalletVersion {
    V5R1,
}

impl Serialize for TonWalletVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("V5R1")
    }
}

impl<'de> Deserialize<'de> for TonWalletVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "V5R1" => Ok(TonWalletVersion::V5R1),
            _ => Err(serde::de::Error::custom("only TON wallet version V5R1 is supported")),
        }
    }
}

/// Network and W5R1 contract parameters defined by a native TON coin configuration.
///
/// All address-affecting fields are explicit. In particular, a configured W5R1 wallet
/// cannot silently change its account address because a default subwallet was inferred.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TonProtocolInfo {
    pub network: TonNetwork,
    pub wallet_version: TonWalletVersion,
    pub workchain: i8,
    pub subwallet_number: TonSubwalletId,
}

impl TonProtocolInfo {
    pub const MAINNET_W5R1: TonProtocolInfo = TonProtocolInfo {
        network: TonNetwork::Mainnet,
        wallet_version: TonWalletVersion::V5R1,
        workchain: 0,
        subwallet_number: TonSubwalletId::DEFAULT,
    };

    pub const fn wallet_params(self) -> TonWalletParams {
        TonWalletParams {
            network: self.network,
            workchain: self.workchain,
            subwallet_id: self.subwallet_number,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_explicit_mainnet_w5r1_parameters() {
        let protocol: TonProtocolInfo = serde_json::from_value(json!({
            "network": "Mainnet",
            "wallet_version": "V5R1",
            "workchain": 0,
            "subwallet_number": 0
        }))
        .unwrap();

        assert_eq!(protocol, TonProtocolInfo::MAINNET_W5R1);
        assert_eq!(protocol.wallet_params(), TonWalletParams::MAINNET_DEFAULT);
    }

    #[test]
    fn rejects_an_unrecognized_wallet_version_or_subwallet() {
        let unsupported_version = serde_json::from_value::<TonProtocolInfo>(json!({
            "network": "Mainnet",
            "wallet_version": "V4R2",
            "workchain": 0,
            "subwallet_number": 0
        }));
        assert!(unsupported_version.is_err());

        let unsupported_subwallet = serde_json::from_value::<TonProtocolInfo>(json!({
            "network": "Mainnet",
            "wallet_version": "V5R1",
            "workchain": 0,
            "subwallet_number": 32768
        }));
        assert!(unsupported_subwallet.is_err());
    }

    #[test]
    fn rejects_implicit_or_unknown_address_parameters() {
        let implicit_subwallet = serde_json::from_value::<TonProtocolInfo>(json!({
            "network": "Mainnet",
            "wallet_version": "V5R1",
            "workchain": 0
        }));
        assert!(implicit_subwallet.is_err());

        let unknown_parameter = serde_json::from_value::<TonProtocolInfo>(json!({
            "network": "Mainnet",
            "wallet_version": "V5R1",
            "workchain": 0,
            "subwallet_number": 0,
            "derivation_path": "m/44'/607'/0'"
        }));
        assert!(unknown_parameter.is_err());
    }
}
