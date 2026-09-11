use super::{TonAddress, TonAddressFormat, TonAmount, TonNetwork};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use common::custom_futures::timeout::FutureTimerExt;
use derive_more::Display;
use serde::Deserialize;
use serde_json::{self as json, Value as Json};
use std::convert::TryFrom;
use std::error::Error;
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

const TON_RPC_TIMEOUT: Duration = Duration::from_secs(15);
const TONCENTER_V2_WALLET_INFORMATION: &str = "getWalletInformation";
const TONCENTER_V2_RUN_GET_METHOD: &str = "runGetMethod";
const TONCENTER_V2_SEND_BOC_RETURN_HASH: &str = "sendBocReturnHash";
const MAX_BOC_BYTES: usize = 1024 * 1024;
#[cfg(target_arch = "wasm32")]
const TON_API_KEY_HEADER: &str = "X-API-Key";

/// A narrow asynchronous client for a TON Center-compatible v2 endpoint.
///
/// `endpoint` must point to the v2 API root, such as
/// `https://toncenter.com/api/v2`. API keys are never included in error text.
pub struct TonRpcClient {
    endpoint: Url,
    api_key: Option<Zeroizing<String>>,
    network: TonNetwork,
}

/// One TON Center-compatible endpoint supplied at activation time.
///
/// API keys are accepted only from the activation request; the static `coins`
/// configuration contains endpoint URLs, never credentials.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TonRpcNode {
    pub url: String,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// A bounded read-only endpoint failover pool.
///
/// Each read attempts every configured endpoint at most once. Broadcasting is
/// deliberately not retried here because a timeout after submission has an
/// unknown chain outcome.
pub struct TonRpcClientPool {
    clients: Vec<TonRpcClient>,
}

impl TonRpcClientPool {
    pub fn new(nodes: Vec<TonRpcNode>, network: TonNetwork) -> Result<Self, TonRpcError> {
        if nodes.is_empty() {
            return Err(TonRpcError::NoEndpoints);
        }
        let clients = nodes
            .into_iter()
            .map(|node| TonRpcClient::new(&node.url, node.api_key, network))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TonRpcClientPool { clients })
    }

    pub async fn wallet_information(&self, address: &TonAddress) -> Result<TonWalletInformation, TonRpcError> {
        let mut last_error = None;
        for client in &self.clients {
            match client.wallet_information(address).await {
                Ok(information) => return Ok(information),
                Err(error) if error.is_retryable() => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(TonRpcError::NoEndpoints))
    }

    pub async fn wallet_information_with_seqno(
        &self,
        address: &TonAddress,
    ) -> Result<TonWalletInformation, TonRpcError> {
        let mut last_error = None;
        for client in &self.clients {
            match client.wallet_information_with_seqno(address).await {
                Ok(information) => return Ok(information),
                Err(error) if error.is_retryable() => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(TonRpcError::NoEndpoints))
    }

    /// Submits through the configured primary endpoint exactly once.
    pub async fn send_boc_return_hash(&self, boc: &[u8]) -> Result<TonBroadcastResult, TonRpcError> {
        let client = self.clients.first().ok_or(TonRpcError::NoEndpoints)?;
        client.send_boc_return_hash(boc).await
    }
}

impl TonRpcClient {
    pub fn new(endpoint: &str, api_key: Option<String>, network: TonNetwork) -> Result<Self, TonRpcError> {
        let mut endpoint = Url::parse(endpoint).map_err(|_| TonRpcError::InvalidEndpoint)?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path().trim_end_matches('/') != "/api/v2"
        {
            return Err(TonRpcError::InvalidEndpoint);
        }
        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }

        Ok(TonRpcClient {
            endpoint,
            api_key: api_key.filter(|key| !key.trim().is_empty()).map(Zeroizing::new),
            network,
        })
    }

    /// Queries balance, account state and seqno without inventing a seqno for an
    /// already active account. Callers must explicitly handle `None` before signing.
    pub async fn wallet_information(&self, address: &TonAddress) -> Result<TonWalletInformation, TonRpcError> {
        let address = self.format_address(address)?;
        let mut url = self
            .endpoint
            .join(TONCENTER_V2_WALLET_INFORMATION)
            .map_err(|_| TonRpcError::InvalidEndpoint)?;
        url.query_pairs_mut().append_pair("address", &address);

        let response = self.get(url).await?;
        parse_wallet_information(&response)
    }

    /// Returns wallet information with a sequence number for an active wallet.
    ///
    /// Some TON Center-compatible providers omit `seqno` from
    /// `getWalletInformation`. Only in that case this makes a separate read-only
    /// `runGetMethod(seqno)` request. An inactive account remains without a seqno.
    pub async fn wallet_information_with_seqno(
        &self,
        address: &TonAddress,
    ) -> Result<TonWalletInformation, TonRpcError> {
        let mut information = self.wallet_information(address).await?;
        if information.account_state == TonAccountState::Active && information.sequence_number.is_none() {
            information.sequence_number = Some(self.get_method_seqno(address).await?);
        }
        Ok(information)
    }

    fn format_address(&self, address: &TonAddress) -> Result<String, TonRpcError> {
        address
            .ensure_network(self.network)
            .map_err(|_| TonRpcError::AddressNetwork)?;
        Ok(address.format(
            TonAddressFormat::Friendly {
                bounceable: false,
                urlsafe: true,
            },
            self.network,
        ))
    }

    async fn get_method_seqno(&self, address: &TonAddress) -> Result<u32, TonRpcError> {
        let address = self.format_address(address)?;
        let mut url = self
            .endpoint
            .join(TONCENTER_V2_RUN_GET_METHOD)
            .map_err(|_| TonRpcError::InvalidEndpoint)?;
        url.query_pairs_mut()
            .append_pair("address", &address)
            .append_pair("method", "seqno")
            .append_pair("stack", "[]");

        let response = self.get(url).await?;
        parse_get_method_seqno(&response)
    }

    /// Broadcasts one already-signed external message and returns the provider's
    /// message reference. A successful response is not proof that the message
    /// was included in a block or that its internal transfer was delivered.
    pub async fn send_boc_return_hash(&self, boc: &[u8]) -> Result<TonBroadcastResult, TonRpcError> {
        validate_boc(boc)?;
        let body =
            json::to_vec(&json::json!({ "boc": BASE64.encode(boc) })).map_err(|_| TonRpcError::InvalidResponse)?;
        let url = self
            .endpoint
            .join(TONCENTER_V2_SEND_BOC_RETURN_HASH)
            .map_err(|_| TonRpcError::InvalidEndpoint)?;

        let response = self.post(url, body).await?;
        parse_broadcast_result(&response)
    }

    async fn get(&self, url: Url) -> Result<Vec<u8>, TonRpcError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use http::header::{HeaderName, HeaderValue};
            use mm2_net::transport::slurp_req;

            let mut request = http::Request::builder()
                .method(http::Method::GET)
                .uri(url.as_str())
                .body(Vec::new())
                .map_err(|_| TonRpcError::InvalidEndpoint)?;
            if let Some(api_key) = self.api_key.as_deref() {
                let header = HeaderValue::from_str(api_key).map_err(|_| TonRpcError::InvalidApiKey)?;
                request
                    .headers_mut()
                    .insert(HeaderName::from_static("x-api-key"), header);
            }

            match Box::pin(slurp_req(request)).timeout(TON_RPC_TIMEOUT).await {
                Ok(Ok((status, _headers, body))) if status.is_success() => Ok(body),
                Ok(Ok((status, _headers, _body))) => Err(TonRpcError::HttpStatus(status.as_u16())),
                Ok(Err(_)) => Err(TonRpcError::Transport),
                Err(_) => Err(TonRpcError::Timeout),
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            use mm2_net::wasm::http::FetchRequest;

            let mut request = FetchRequest::get(url.as_str()).cors();
            if let Some(api_key) = self.api_key.as_deref() {
                request = request.header(TON_API_KEY_HEADER, api_key);
            }

            match Box::pin(request.request_str()).timeout(TON_RPC_TIMEOUT).await {
                Ok(Ok((status, body))) if status.is_success() => Ok(body.into_bytes()),
                Ok(Ok((status, _body))) => Err(TonRpcError::HttpStatus(status.as_u16())),
                Ok(Err(_)) => Err(TonRpcError::Transport),
                Err(_) => Err(TonRpcError::Timeout),
            }
        }
    }

    async fn post(&self, url: Url, body: Vec<u8>) -> Result<Vec<u8>, TonRpcError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use http::header::{HeaderName, HeaderValue, CONTENT_TYPE};
            use mm2_net::transport::slurp_req;

            let mut request = http::Request::builder()
                .method(http::Method::POST)
                .uri(url.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(body)
                .map_err(|_| TonRpcError::InvalidEndpoint)?;
            if let Some(api_key) = self.api_key.as_deref() {
                let header = HeaderValue::from_str(api_key).map_err(|_| TonRpcError::InvalidApiKey)?;
                request
                    .headers_mut()
                    .insert(HeaderName::from_static("x-api-key"), header);
            }

            match Box::pin(slurp_req(request)).timeout(TON_RPC_TIMEOUT).await {
                Ok(Ok((status, _headers, body))) if status.is_success() => Ok(body),
                Ok(Ok((status, _headers, _body))) => Err(TonRpcError::HttpStatus(status.as_u16())),
                Ok(Err(_)) => Err(TonRpcError::Transport),
                Err(_) => Err(TonRpcError::Timeout),
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            use mm2_net::wasm::http::FetchRequest;

            let body = String::from_utf8(body).map_err(|_| TonRpcError::InvalidResponse)?;
            let mut request = FetchRequest::post(url.as_str())
                .cors()
                .body_utf8(body)
                .header("Content-Type", "application/json");
            if let Some(api_key) = self.api_key.as_deref() {
                request = request.header(TON_API_KEY_HEADER, api_key);
            }

            match Box::pin(request.request_str()).timeout(TON_RPC_TIMEOUT).await {
                Ok(Ok((status, body))) if status.is_success() => Ok(body.into_bytes()),
                Ok(Ok((status, _body))) => Err(TonRpcError::HttpStatus(status.as_u16())),
                Ok(Err(_)) => Err(TonRpcError::Transport),
                Err(_) => Err(TonRpcError::Timeout),
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TonAccountState {
    Active,
    Uninitialized,
    Frozen,
    Unknown,
}

/// Account data needed by balance and withdrawal code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonWalletInformation {
    pub balance: TonAmount,
    pub account_state: TonAccountState,
    /// Missing for some provider responses. In particular, it is never coerced to
    /// zero for an active account.
    pub sequence_number: Option<u32>,
    pub wallet_type: Option<String>,
}

/// The provider's reference to an externally submitted message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonBroadcastResult {
    pub message_hash: String,
}

#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum TonRpcError {
    #[display(fmt = "No TON RPC endpoints are configured")]
    NoEndpoints,
    #[display(fmt = "Invalid TON RPC endpoint")]
    InvalidEndpoint,
    #[display(fmt = "Invalid TON RPC API key")]
    InvalidApiKey,
    #[display(fmt = "TON address network does not match the configured network")]
    AddressNetwork,
    #[display(fmt = "TON RPC request timed out")]
    Timeout,
    #[display(fmt = "TON RPC transport failed")]
    Transport,
    #[display(fmt = "TON RPC returned HTTP status {_0}")]
    HttpStatus(u16),
    #[display(fmt = "TON RPC returned an invalid response")]
    InvalidResponse,
    #[display(fmt = "TON BOC is empty or exceeds the maximum supported size")]
    InvalidBoc,
    #[display(fmt = "TON RPC rejected the request: {_0}")]
    Remote(String),
}

impl Error for TonRpcError {}

impl TonRpcError {
    fn is_retryable(&self) -> bool {
        matches!(self, TonRpcError::Timeout | TonRpcError::Transport)
            || matches!(self, TonRpcError::HttpStatus(status) if *status == 429 || *status >= 500)
    }
}

fn parse_wallet_information(bytes: &[u8]) -> Result<TonWalletInformation, TonRpcError> {
    let response: Json = json::from_slice(bytes).map_err(|_| TonRpcError::InvalidResponse)?;
    let result = parse_success_result(&response)?;
    let balance = parse_u64(result.get("balance"))?;
    let sequence_number = match result.get("seqno") {
        Some(value) => Some(parse_u32(value)?),
        None => None,
    };
    let account_state = match result.get("account_state").and_then(Json::as_str) {
        Some("active") => TonAccountState::Active,
        Some("uninitialized") | Some("nonexist") => TonAccountState::Uninitialized,
        Some("frozen") => TonAccountState::Frozen,
        _ => TonAccountState::Unknown,
    };
    let wallet_type = result.get("wallet_type").and_then(Json::as_str).map(str::to_owned);

    Ok(TonWalletInformation {
        balance: TonAmount::from_nano(balance),
        account_state,
        sequence_number,
        wallet_type,
    })
}

fn parse_success_result(response: &Json) -> Result<&Json, TonRpcError> {
    if response.get("ok").and_then(Json::as_bool) == Some(true) {
        return response.get("result").ok_or(TonRpcError::InvalidResponse);
    }

    let message = response
        .get("error")
        .or_else(|| response.get("result"))
        .and_then(Json::as_str)
        .map(limit_error_message)
        .unwrap_or_else(|| "unknown provider error".to_owned());
    Err(TonRpcError::Remote(message))
}

fn parse_u64(value: Option<&Json>) -> Result<u64, TonRpcError> {
    match value {
        Some(Json::String(value)) if value.bytes().all(|byte| byte.is_ascii_digit()) => {
            value.parse().map_err(|_| TonRpcError::InvalidResponse)
        },
        Some(Json::Number(value)) => value.as_u64().ok_or(TonRpcError::InvalidResponse),
        _ => Err(TonRpcError::InvalidResponse),
    }
}

fn parse_u32(value: &Json) -> Result<u32, TonRpcError> {
    u32::try_from(parse_u64(Some(value))?).map_err(|_| TonRpcError::InvalidResponse)
}

fn parse_get_method_seqno(bytes: &[u8]) -> Result<u32, TonRpcError> {
    let response: Json = json::from_slice(bytes).map_err(|_| TonRpcError::InvalidResponse)?;
    let result = parse_success_result(&response)?;
    if result.get("exit_code").and_then(Json::as_i64) != Some(0) {
        return Err(TonRpcError::InvalidResponse);
    }

    let value = result
        .get("stack")
        .and_then(Json::as_array)
        .and_then(|stack| stack.first())
        .and_then(Json::as_array)
        .filter(|entry| entry.len() == 2 && entry.first().and_then(Json::as_str) == Some("num"))
        .and_then(|entry| entry.get(1))
        .and_then(Json::as_str)
        .and_then(|value| value.strip_prefix("0x"))
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .ok_or(TonRpcError::InvalidResponse)?;
    Ok(value)
}

fn parse_broadcast_result(bytes: &[u8]) -> Result<TonBroadcastResult, TonRpcError> {
    let response: Json = json::from_slice(bytes).map_err(|_| TonRpcError::InvalidResponse)?;
    let result = parse_success_result(&response)?;
    let message_hash = result
        .get("hash")
        .and_then(Json::as_str)
        .filter(|hash| !hash.is_empty() && hash.len() <= 128 && hash.trim() == *hash)
        .map(str::to_owned)
        .ok_or(TonRpcError::InvalidResponse)?;
    Ok(TonBroadcastResult { message_hash })
}

fn validate_boc(boc: &[u8]) -> Result<(), TonRpcError> {
    if boc.is_empty() || boc.len() > MAX_BOC_BYTES {
        return Err(TonRpcError::InvalidBoc);
    }
    Ok(())
}

fn limit_error_message(message: &str) -> String {
    message.chars().take(256).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_active_wallet_without_losing_precision() {
        let info = parse_wallet_information(
            br#"{"ok":true,"result":{"balance":"1776019966","account_state":"active","seqno":3,"wallet_type":"v5r1"}}"#,
        )
        .unwrap();

        assert_eq!(info.balance, TonAmount::from_nano(1_776_019_966));
        assert_eq!(info.account_state, TonAccountState::Active);
        assert_eq!(info.sequence_number, Some(3));
        assert_eq!(info.wallet_type.as_deref(), Some("v5r1"));
    }

    #[test]
    fn does_not_turn_a_missing_active_seqno_into_zero() {
        let info = parse_wallet_information(br#"{"ok":true,"result":{"balance":0,"account_state":"active"}}"#).unwrap();

        assert_eq!(info.sequence_number, None);
    }

    #[test]
    fn parses_seqno_from_a_successful_get_method_response() {
        assert_eq!(
            parse_get_method_seqno(br#"{"ok":true,"result":{"exit_code":0,"stack":[["num","0x2a"]]}}"#),
            Ok(42),
        );
    }

    #[test]
    fn rejects_missing_or_failed_get_method_seqno() {
        assert_eq!(
            parse_get_method_seqno(br#"{"ok":true,"result":{"exit_code":11,"stack":[["num","0x2a"]]}}"#),
            Err(TonRpcError::InvalidResponse),
        );
        assert_eq!(
            parse_get_method_seqno(br#"{"ok":true,"result":{"exit_code":0,"stack":[["num","42"]]}}"#),
            Err(TonRpcError::InvalidResponse),
        );
    }

    #[test]
    fn parses_a_broadcast_message_reference_without_equating_it_to_a_transaction() {
        assert_eq!(
            parse_broadcast_result(br#"{"ok":true,"result":{"hash":"D3kz2hH78yEYpw=="}}"#),
            Ok(TonBroadcastResult {
                message_hash: "D3kz2hH78yEYpw==".to_owned(),
            }),
        );
    }

    #[test]
    fn rejects_missing_or_unbounded_broadcast_references() {
        assert_eq!(
            parse_broadcast_result(br#"{"ok":true,"result":{}}"#),
            Err(TonRpcError::InvalidResponse),
        );
        assert_eq!(validate_boc(&[]), Err(TonRpcError::InvalidBoc));
        assert_eq!(validate_boc(&vec![0; MAX_BOC_BYTES + 1]), Err(TonRpcError::InvalidBoc));
    }

    #[test]
    fn classifies_uninitialized_and_rejects_bad_provider_envelopes() {
        let uninitialized =
            parse_wallet_information(br#"{"ok":true,"result":{"balance":"0","account_state":"uninitialized"}}"#)
                .unwrap();
        assert_eq!(uninitialized.account_state, TonAccountState::Uninitialized);

        assert_eq!(
            parse_wallet_information(br#"{"ok":false,"error":"Ratelimit exceeded"}"#),
            Err(TonRpcError::Remote("Ratelimit exceeded".to_owned())),
        );
        assert_eq!(
            parse_wallet_information(br#"{"ok":true,"result":{"balance":"-1"}}"#),
            Err(TonRpcError::InvalidResponse),
        );
    }

    #[test]
    fn validates_the_v2_api_root_without_exposing_credentials() {
        assert!(TonRpcClient::new("https://toncenter.com/api/v2", None, TonNetwork::Mainnet).is_ok());
        assert!(matches!(
            TonRpcClient::new("https://user:password@toncenter.com/api/v2", None, TonNetwork::Mainnet),
            Err(TonRpcError::InvalidEndpoint),
        ));
        assert!(matches!(
            TonRpcClient::new("ftp://toncenter.com/api/v2", None, TonNetwork::Mainnet),
            Err(TonRpcError::InvalidEndpoint),
        ));
        assert!(matches!(
            TonRpcClient::new("https://toncenter.com", None, TonNetwork::Mainnet),
            Err(TonRpcError::InvalidEndpoint),
        ));
    }

    #[test]
    fn validates_a_non_empty_pool_and_classifies_failover_errors() {
        assert!(matches!(
            TonRpcClientPool::new(Vec::new(), TonNetwork::Mainnet),
            Err(TonRpcError::NoEndpoints)
        ));
        assert!(TonRpcError::Timeout.is_retryable());
        assert!(TonRpcError::HttpStatus(429).is_retryable());
        assert!(TonRpcError::HttpStatus(503).is_retryable());
        assert!(!TonRpcError::HttpStatus(400).is_retryable());
        assert!(!TonRpcError::InvalidResponse.is_retryable());
    }
}
