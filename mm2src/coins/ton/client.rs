use super::{TonAddress, TonAddressFormat, TonAmount, TonNetwork};
use common::custom_futures::timeout::FutureTimerExt;
use derive_more::Display;
use serde_json::{self as json, Value as Json};
use std::convert::TryFrom;
use std::error::Error;
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

const TON_RPC_TIMEOUT: Duration = Duration::from_secs(15);
const TONCENTER_V2_WALLET_INFORMATION: &str = "getWalletInformation";
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
        address
            .ensure_network(self.network)
            .map_err(|_| TonRpcError::AddressNetwork)?;
        let mut url = self
            .endpoint
            .join(TONCENTER_V2_WALLET_INFORMATION)
            .map_err(|_| TonRpcError::InvalidEndpoint)?;
        url.query_pairs_mut().append_pair(
            "address",
            &address.format(
                TonAddressFormat::Friendly {
                    bounceable: false,
                    urlsafe: true,
                },
                self.network,
            ),
        );

        let response = self.get(url).await?;
        parse_wallet_information(&response)
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

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonRpcError {
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
    #[display(fmt = "TON RPC rejected the request: {_0}")]
    Remote(String),
}

impl Error for TonRpcError {}

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
}
