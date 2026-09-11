use super::{
    build_fee_estimate_request, build_signed_transfer, TonAccountStateError, TonAccountTransaction, TonAddress,
    TonAmount, TonFeeEstimate, TonKeyPolicyError, TonProtocolInfo, TonRpcClientPool, TonRpcError, TonRpcNode,
    TonSignedTransfer, TonSigningSeed, TonTransferError, TonTransferRequest, TonWalletError, TonWalletInformation,
};
use crate::PrivKeyBuildPolicy;
use derive_more::Display;
use serde::Deserialize;
use serde_json::{self as json, Value as Json};
use std::error::Error;

/// Validated native-GRAM configuration extracted from the shared `coins` artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonCoinConfig {
    pub ticker: String,
    pub required_confirmations: u64,
    pub protocol: TonProtocolInfo,
}

impl TonCoinConfig {
    pub fn from_json(value: Json) -> Result<Self, TonActivationError> {
        let config: TonCoinConfigJson =
            json::from_value(value).map_err(|_| TonActivationError::InvalidConfiguration)?;
        if config.coin.trim().is_empty() || config.decimals != super::TON_DECIMALS || config.required_confirmations == 0
        {
            return Err(TonActivationError::InvalidConfiguration);
        }
        if !config.wallet_only {
            return Err(TonActivationError::WalletOnlyRequired);
        }
        if config.protocol.protocol_type != "TON" {
            return Err(TonActivationError::UnsupportedProtocol);
        }

        Ok(TonCoinConfig {
            ticker: config.coin,
            required_confirmations: config.required_confirmations,
            protocol: config.protocol.protocol_data,
        })
    }
}

#[derive(Deserialize)]
struct TonCoinConfigJson {
    coin: String,
    decimals: u8,
    required_confirmations: u64,
    wallet_only: bool,
    protocol: TonProtocolEnvelope,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TonProtocolEnvelope {
    #[serde(rename = "type")]
    protocol_type: String,
    protocol_data: TonProtocolInfo,
}

/// Request fields required to initialize native GRAM.
///
/// Nodes are supplied by the caller or generated application configuration.
/// Credentials belong only here, never in the shared `coins` repository.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TonActivationRequest {
    #[serde(alias = "rpc_nodes")]
    pub nodes: Vec<TonRpcNode>,
    #[serde(default)]
    pub required_confirmations: Option<u64>,
    #[serde(default)]
    pub tx_history: bool,
}

impl TonActivationRequest {
    /// Extracts the TON-specific fields from the legacy `enable` request.
    ///
    /// `nodes` is preferred; `rpc_nodes` is also accepted so the node section
    /// from the companion `coins/ton/GRAM` artifact can be passed unchanged.
    /// The static coin entry intentionally contains no provider endpoint or
    /// credential, therefore an activation without nodes is rejected.
    pub fn from_legacy_req(req: &Json) -> Result<Self, TonActivationError> {
        let nodes = match (req.get("nodes"), req.get("rpc_nodes")) {
            (Some(_), Some(_)) => return Err(TonActivationError::AmbiguousEndpoints),
            (Some(nodes), None) | (None, Some(nodes)) => {
                json::from_value(nodes.clone()).map_err(|_| TonActivationError::InvalidEndpoints)?
            },
            (None, None) => return Err(TonActivationError::MissingEndpoints),
        };
        let required_confirmations = json::from_value(req["required_confirmations"].clone())
            .map_err(|_| TonActivationError::InvalidRequiredConfirmations)?;

        Ok(TonActivationRequest {
            nodes,
            required_confirmations,
            tx_history: req["tx_history"].as_bool().unwrap_or(false),
        })
    }
}

/// Wallet identity and RPC access created before registering a TON coin.
///
/// This context owns the zeroizing signing seed. It has no side effects: the
/// future activation service must query account state and register the final
/// `TonCoin` only after all validation succeeds.
pub struct TonWalletContext {
    config: TonCoinConfig,
    signing_seed: TonSigningSeed,
    rpc: TonRpcClientPool,
    required_confirmations: u64,
    tx_history: bool,
}

/// A freshly signed transfer together with the state used to authorize it.
/// No private key material is retained in this value.
pub struct TonPreparedTransfer {
    pub signed: TonSignedTransfer,
    pub fee: TonFeeEstimate,
    pub available_balance: TonAmount,
    pub amount: TonAmount,
}

impl TonWalletContext {
    pub fn new(
        config: TonCoinConfig,
        request: TonActivationRequest,
        key_policy: PrivKeyBuildPolicy,
    ) -> Result<Self, TonActivationError> {
        if request.tx_history {
            return Err(TonActivationError::TransactionHistoryUnsupported);
        }
        let required_confirmations = request.required_confirmations.unwrap_or(config.required_confirmations);
        if required_confirmations == 0 {
            return Err(TonActivationError::InvalidRequiredConfirmations);
        }
        let signing_seed = TonSigningSeed::from_priv_key_policy(key_policy).map_err(TonActivationError::KeyPolicy)?;
        // Derive once during preflight so invalid W5 construction fails before any
        // network request or future coin registration. The context retains only
        // the seed and derives a short-lived address value for each caller.
        signing_seed
            .address(config.protocol.wallet_params())
            .map_err(TonActivationError::WalletConstruction)?;
        let rpc = TonRpcClientPool::new(request.nodes, config.protocol.network).map_err(TonActivationError::Rpc)?;

        Ok(TonWalletContext {
            config,
            signing_seed,
            rpc,
            required_confirmations,
            tx_history: request.tx_history,
        })
    }

    pub fn ticker(&self) -> &str {
        &self.config.ticker
    }

    pub fn address(&self) -> Result<TonAddress, TonActivationError> {
        self.signing_seed
            .address(self.config.protocol.wallet_params())
            .map_err(TonActivationError::WalletConstruction)
    }

    pub fn protocol(&self) -> TonProtocolInfo {
        self.config.protocol
    }

    pub fn required_confirmations(&self) -> u64 {
        self.required_confirmations
    }

    pub fn tx_history_enabled(&self) -> bool {
        self.tx_history
    }

    pub async fn wallet_information(&self) -> Result<TonWalletInformation, TonActivationError> {
        let address = self.address()?;
        self.rpc
            .wallet_information_with_seqno(&address)
            .await
            .map_err(TonActivationError::Rpc)
    }

    pub async fn current_block(&self) -> Result<u64, TonActivationError> {
        self.rpc.current_block().await.map_err(TonActivationError::Rpc)
    }

    /// Retrieves a bounded newest-first snapshot for a future history or
    /// confirmation worker. It does not enable persistence by itself.
    pub async fn account_transactions(&self, limit: u8) -> Result<Vec<TonAccountTransaction>, TonActivationError> {
        let address = self.address()?;
        self.rpc
            .account_transactions(&address, limit)
            .await
            .map_err(TonActivationError::Rpc)
    }

    pub async fn message_masterchain_seqno(&self, message_hash: &str) -> Result<Option<u64>, TonActivationError> {
        self.rpc
            .message_masterchain_seqno(message_hash)
            .await
            .map_err(TonActivationError::Rpc)
    }

    pub async fn message_outcome(
        &self,
        message_hash: &str,
    ) -> Result<Option<super::TonMessageOutcome>, TonActivationError> {
        self.rpc
            .message_outcome(message_hash)
            .await
            .map_err(TonActivationError::Rpc)
    }

    /// Broadcasts a previously signed external BOC exactly once through the
    /// primary endpoint. A transport timeout has an unknown chain outcome and
    /// is deliberately not retried.
    pub async fn broadcast_boc(&self, boc: &[u8]) -> Result<String, TonActivationError> {
        self.rpc
            .send_boc_return_hash(boc)
            .await
            .map(|result| result.message_hash)
            .map_err(TonActivationError::Rpc)
    }

    /// Checks the account state before a coin is registered. An uninitialized
    /// account is valid: its first W5 transfer will deploy the wallet contract.
    pub async fn validate_account_state(&self) -> Result<TonWalletInformation, TonActivationError> {
        let information = self.wallet_information().await?;
        self.validate_wallet_information(&information)?;
        Ok(information)
    }

    /// Reads fresh state, signs an exact W5 transfer and obtains its simulated
    /// source fee. This does not broadcast or mutate local state.
    ///
    /// The TON wallet object is intentionally confined to this method because
    /// the upstream library keeps a signing key inside it.
    pub async fn prepare_transfer(
        &self,
        recipient: TonAddress,
        amount: TonAmount,
        expire_at: u32,
    ) -> Result<TonPreparedTransfer, TonActivationError> {
        let information = self.wallet_information().await?;
        let state = self.validate_wallet_information(&information)?;
        let wallet = self
            .signing_seed
            .wallet(self.config.protocol.wallet_params())
            .map_err(TonActivationError::WalletConstruction)?;
        let request = TonTransferRequest {
            bounceable: recipient.is_bounceable().unwrap_or(false),
            recipient,
            amount,
            sequence_number: state.sequence_number(),
            expire_at,
            deploy_wallet: state.deploy_wallet(),
        };
        let fee_request = build_fee_estimate_request(&wallet, self.config.protocol.network, &request)
            .map_err(TonActivationError::Transfer)?;
        let signed = build_signed_transfer(&wallet, self.config.protocol.network, &request)
            .map_err(TonActivationError::Transfer)?;
        let fee = self
            .rpc
            .estimate_fee(&fee_request)
            .await
            .map_err(TonActivationError::Rpc)?;

        Ok(TonPreparedTransfer {
            signed,
            fee,
            available_balance: information.balance,
            amount,
        })
    }

    fn validate_wallet_information(
        &self,
        information: &TonWalletInformation,
    ) -> Result<super::TonTransferState, TonActivationError> {
        let state = information.transfer_state().map_err(TonActivationError::AccountState)?;
        if information.account_state == super::TonAccountState::Active
            && !is_w5r1_wallet_type(information.wallet_type.as_deref())
        {
            return Err(TonActivationError::IncompatibleActiveWallet);
        }
        Ok(state)
    }
}

fn is_w5r1_wallet_type(wallet_type: Option<&str>) -> bool {
    let normalized: String = wallet_type
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    matches!(normalized.as_str(), "v5r1" | "walletv5r1")
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonActivationError {
    #[display(fmt = "Invalid TON coin configuration")]
    InvalidConfiguration,
    #[display(fmt = "GRAM must be configured as a wallet-only coin")]
    WalletOnlyRequired,
    #[display(fmt = "Coin configuration is not a TON protocol")]
    UnsupportedProtocol,
    #[display(fmt = "TON required confirmations must be greater than zero")]
    InvalidRequiredConfirmations,
    #[display(fmt = "TON activation requires a non-empty nodes or rpc_nodes array")]
    MissingEndpoints,
    #[display(fmt = "TON activation must use either nodes or rpc_nodes, not both")]
    AmbiguousEndpoints,
    #[display(fmt = "TON activation nodes are invalid")]
    InvalidEndpoints,
    #[display(fmt = "TON transaction history is not implemented")]
    TransactionHistoryUnsupported,
    #[display(fmt = "Unable to select a TON wallet key source: {_0}")]
    KeyPolicy(TonKeyPolicyError),
    #[display(fmt = "Unable to construct TON wallet identity: {_0}")]
    WalletConstruction(TonWalletError),
    #[display(fmt = "Unable to initialize TON RPC: {_0}")]
    Rpc(TonRpcError),
    #[display(fmt = "TON account cannot be used: {_0}")]
    AccountState(TonAccountStateError),
    #[display(fmt = "Active TON account is not a compatible W5R1 wallet")]
    IncompatibleActiveWallet,
    #[display(fmt = "Unable to construct TON transfer: {_0}")]
    Transfer(TonTransferError),
}

impl Error for TonActivationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ton::TonNetwork;
    use crate::{IguanaPrivKey, PrivKeyBuildPolicy};

    fn config() -> Json {
        json::json!({
            "coin": "GRAM",
            "decimals": 9,
            "required_confirmations": 1,
            "wallet_only": true,
            "protocol": {
                "type": "TON",
                "protocol_data": {
                    "network": "Mainnet",
                    "wallet_version": "V5R1",
                    "workchain": 0,
                    "subwallet_number": 0
                }
            }
        })
    }

    fn request() -> TonActivationRequest {
        TonActivationRequest {
            nodes: vec![TonRpcNode {
                url: "https://toncenter.com/api/v2".to_owned(),
                api_key: None,
            }],
            required_confirmations: None,
            tx_history: false,
        }
    }

    #[test]
    fn extracts_exactly_one_legacy_node_source() {
        let request = TonActivationRequest::from_legacy_req(&json::json!({
            "rpc_nodes": [{"url": "https://toncenter.com/api/v2"}],
            "required_confirmations": 2,
        }))
        .unwrap();
        assert_eq!(request.nodes.len(), 1);
        assert_eq!(request.required_confirmations, Some(2));

        let task_request: TonActivationRequest = serde_json::from_value(json::json!({
            "rpc_nodes": [{"url": "https://toncenter.com/api/v2"}]
        }))
        .unwrap();
        assert_eq!(task_request.nodes.len(), 1);

        assert!(matches!(
            TonActivationRequest::from_legacy_req(&json::json!({})),
            Err(TonActivationError::MissingEndpoints)
        ));
        assert!(matches!(
            TonActivationRequest::from_legacy_req(&json::json!({
                "nodes": [], "rpc_nodes": []
            })),
            Err(TonActivationError::AmbiguousEndpoints)
        ));
    }

    #[test]
    fn accepts_the_native_gram_wallet_only_configuration() {
        let config = TonCoinConfig::from_json(config()).unwrap();
        assert_eq!(config.ticker, "GRAM");
        assert_eq!(config.protocol.network, TonNetwork::Mainnet);
    }

    #[test]
    fn rejects_non_wallet_only_or_non_native_gram_configuration() {
        let mut not_wallet_only = config();
        not_wallet_only["wallet_only"] = Json::Bool(false);
        assert_eq!(
            TonCoinConfig::from_json(not_wallet_only),
            Err(TonActivationError::WalletOnlyRequired),
        );

        let mut wrong_decimals = config();
        wrong_decimals["decimals"] = Json::from(8);
        assert_eq!(
            TonCoinConfig::from_json(wrong_decimals),
            Err(TonActivationError::InvalidConfiguration),
        );
    }

    #[test]
    fn builds_iguana_context_without_network_side_effects() {
        let context = TonWalletContext::new(
            TonCoinConfig::from_json(config()).unwrap(),
            request(),
            PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from([0x42; 32])),
        )
        .unwrap();

        assert_eq!(context.ticker(), "GRAM");
        assert_eq!(context.required_confirmations(), 1);
        assert!(!context.tx_history_enabled());
        assert_eq!(
            context.address().unwrap().format(
                super::super::TonAddressFormat::Friendly {
                    bounceable: false,
                    urlsafe: true,
                },
                TonNetwork::Mainnet,
            ),
            "UQA_O1iT-mrBM2FBjVKUiM9O6Qv--yzmD9F8bIXS3aq6jrad",
        );
    }

    #[test]
    fn rejects_history_until_ton_history_is_implemented() {
        let mut request = request();
        request.tx_history = true;
        assert!(matches!(
            TonWalletContext::new(
                TonCoinConfig::from_json(config()).unwrap(),
                request,
                PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from([0x42; 32])),
            ),
            Err(TonActivationError::TransactionHistoryUnsupported)
        ));
    }

    #[test]
    fn recognizes_only_w5r1_active_wallet_type_labels() {
        assert!(is_w5r1_wallet_type(Some("v5r1")));
        assert!(is_w5r1_wallet_type(Some("wallet v5 r1")));
        assert!(!is_w5r1_wallet_type(None));
        assert!(!is_w5r1_wallet_type(Some("v4r2")));
    }
}
