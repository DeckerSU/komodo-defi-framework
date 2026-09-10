use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::error::Error;
use tonlib_core::TonAddress as InnerTonAddress;

/// TON network encoded by a user-friendly address tag or selected by coin configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TonNetwork {
    Mainnet,
    Testnet,
}

impl TonNetwork {
    pub const fn is_mainnet(self) -> bool {
        matches!(self, TonNetwork::Mainnet)
    }

    pub const fn global_id(self) -> i32 {
        match self {
            TonNetwork::Mainnet => -239,
            TonNetwork::Testnet => -3,
        }
    }
}

/// Formatting options for a TON address.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TonAddressFormat {
    Raw,
    Friendly { bounceable: bool, urlsafe: bool },
}

/// A validated TON account address together with an optional friendly-address network tag.
#[derive(Clone)]
pub struct TonAddress {
    inner: InnerTonAddress,
    tagged_network: Option<TonNetwork>,
    bounceable: Option<bool>,
}

impl PartialEq for TonAddress {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for TonAddress {}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonAddressError {
    #[display(fmt = "Invalid TON address")]
    InvalidAddress,
    #[display(fmt = "TON address is for {actual:?}, expected {expected:?}")]
    NetworkMismatch { actual: TonNetwork, expected: TonNetwork },
}

impl Error for TonAddressError {}

impl TonAddress {
    pub fn parse(input: &str) -> Result<Self, TonAddressError> {
        let (inner, tagged_network, bounceable) = if input.len() == 48 {
            let (inner, non_bounceable, testnet) = if input.contains(['-', '_']) {
                InnerTonAddress::from_base64_url_flags(input)
            } else {
                InnerTonAddress::from_base64_std_flags(input)
            }
            .map_err(|_| TonAddressError::InvalidAddress)?;
            let network = if testnet {
                TonNetwork::Testnet
            } else {
                TonNetwork::Mainnet
            };
            (inner, Some(network), Some(!non_bounceable))
        } else {
            let inner = InnerTonAddress::from_hex_str(input).map_err(|_| TonAddressError::InvalidAddress)?;
            (inner, None, None)
        };
        Ok(TonAddress {
            inner,
            tagged_network,
            bounceable,
        })
    }

    pub fn from_inner(inner: InnerTonAddress) -> Self {
        TonAddress {
            inner,
            tagged_network: None,
            bounceable: None,
        }
    }

    pub fn inner(&self) -> &InnerTonAddress {
        &self.inner
    }

    pub const fn tagged_network(&self) -> Option<TonNetwork> {
        self.tagged_network
    }

    pub const fn is_bounceable(&self) -> Option<bool> {
        self.bounceable
    }

    pub fn ensure_network(&self, expected: TonNetwork) -> Result<(), TonAddressError> {
        if let Some(actual) = self.tagged_network {
            if actual != expected {
                return Err(TonAddressError::NetworkMismatch { actual, expected });
            }
        }
        Ok(())
    }

    pub fn format(&self, format: TonAddressFormat, network: TonNetwork) -> String {
        match format {
            TonAddressFormat::Raw => self.inner.to_hex(),
            TonAddressFormat::Friendly { bounceable, urlsafe } => {
                if urlsafe {
                    self.inner.to_base64_url_flags(!bounceable, !network.is_mainnet())
                } else {
                    self.inner.to_base64_std_flags(!bounceable, !network.is_mainnet())
                }
            },
        }
    }
}

impl std::fmt::Debug for TonAddress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TonAddress")
            .field("raw", &self.inner.to_hex())
            .field("tagged_network", &self.tagged_network)
            .field("bounceable", &self.bounceable)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAINNET_NON_BOUNCEABLE: &str = "UQBYGTsWwxh00p3Fq_EdwzQ2uRzuptfxP5crEOsfRT6zDOS4";

    #[test]
    fn parses_and_formats_a_user_friendly_address() {
        let address = TonAddress::parse(MAINNET_NON_BOUNCEABLE).unwrap();

        assert_eq!(address.tagged_network(), Some(TonNetwork::Mainnet));
        assert_eq!(address.is_bounceable(), Some(false));
        assert_eq!(
            address.format(
                TonAddressFormat::Friendly {
                    bounceable: false,
                    urlsafe: true,
                },
                TonNetwork::Mainnet,
            ),
            MAINNET_NON_BOUNCEABLE,
        );
        assert_eq!(address.format(TonAddressFormat::Raw, TonNetwork::Mainnet).len(), 66);
    }

    #[test]
    fn rejects_a_friendly_address_for_the_other_network() {
        let address = TonAddress::parse(MAINNET_NON_BOUNCEABLE).unwrap();
        let testnet = address.format(
            TonAddressFormat::Friendly {
                bounceable: false,
                urlsafe: true,
            },
            TonNetwork::Testnet,
        );

        let testnet_address = TonAddress::parse(&testnet).unwrap();
        assert_eq!(
            testnet_address.ensure_network(TonNetwork::Mainnet),
            Err(TonAddressError::NetworkMismatch {
                actual: TonNetwork::Testnet,
                expected: TonNetwork::Mainnet,
            }),
        );
    }

    #[test]
    fn rejects_an_invalid_friendly_address() {
        assert_eq!(
            TonAddress::parse("UQBYGTsWwxh00p3Fq_EdwzQ2uRzuptfxP5crEOsfRT6zDOS5"),
            Err(TonAddressError::InvalidAddress),
        );
    }

    #[test]
    fn preserves_raw_and_standard_friendly_forms() {
        let address = TonAddress::parse(MAINNET_NON_BOUNCEABLE).unwrap();
        let raw = address.format(TonAddressFormat::Raw, TonNetwork::Mainnet);
        let standard = address.format(
            TonAddressFormat::Friendly {
                bounceable: false,
                urlsafe: false,
            },
            TonNetwork::Mainnet,
        );

        assert_eq!(TonAddress::parse(&raw).unwrap(), address);
        assert_eq!(TonAddress::parse(&standard).unwrap(), address);
    }
}
