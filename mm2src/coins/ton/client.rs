use super::{TonAddress, TonAddressFormat, TonAmount, TonNetwork};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use common::{custom_futures::timeout::FutureTimerExt, executor::Timer, now_ms};
use derive_more::Display;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{self as json, Value as Json};
use std::convert::TryFrom;
use std::error::Error;
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

const TON_RPC_TIMEOUT: Duration = Duration::from_secs(15);
const TONCENTER_V2_WALLET_INFORMATION: &str = "getWalletInformation";
const TONCENTER_V2_MASTERCHAIN_INFO: &str = "getMasterchainInfo";
const TONCENTER_V2_ESTIMATE_FEE: &str = "estimateFee";
const TONCENTER_V2_GET_TRANSACTIONS: &str = "getTransactions";
const TONCENTER_V2_RUN_GET_METHOD: &str = "runGetMethod";
const TONCENTER_V2_SEND_BOC_RETURN_HASH: &str = "sendBocReturnHash";
const TONCENTER_V3_TRANSACTIONS_BY_MESSAGE: &str = "/api/v3/transactionsByMessage";
const MAX_BOC_BYTES: usize = 1024 * 1024;
const MAX_TRANSACTION_PAGE_SIZE: u8 = 100;
/// TON Center's anonymous API allowance is one request per second. A client
/// supplied API key uses the provider's authenticated allowance instead.
const PUBLIC_API_REQUEST_INTERVAL: Duration = Duration::from_secs(1);
const PUBLIC_RATE_LIMIT_RETRY_DELAY: Duration = Duration::from_secs(1);
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
    /// Wall-clock millisecond at which the next anonymous request may start.
    /// `common::now_ms` is available on native and wasm targets; `Instant` is
    /// not implemented by Rust's wasm32-unknown-unknown standard library.
    next_public_request_at: Mutex<Option<u64>>,
}

/// One TON Center-compatible endpoint supplied at activation time.
///
/// API keys are accepted only from the activation request; the static `coins`
/// configuration contains endpoint URLs, never credentials.
#[derive(Clone, Deserialize)]
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

    pub async fn current_block(&self) -> Result<u64, TonRpcError> {
        let mut last_error = None;
        for client in &self.clients {
            match client.current_block().await {
                Ok(block) => return Ok(block),
                Err(error) if error.is_retryable() => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(TonRpcError::NoEndpoints))
    }

    pub async fn estimate_fee(&self, request: &TonFeeEstimateRequest) -> Result<TonFeeEstimate, TonRpcError> {
        let mut last_error = None;
        for client in &self.clients {
            match client.estimate_fee(request).await {
                Ok(fee) => return Ok(fee),
                Err(error) if error.is_retryable() => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(TonRpcError::NoEndpoints))
    }

    pub async fn account_transactions(
        &self,
        address: &TonAddress,
        limit: u8,
    ) -> Result<Vec<TonAccountTransaction>, TonRpcError> {
        self.account_transactions_page(address, limit, None).await
    }

    pub async fn account_transactions_page(
        &self,
        address: &TonAddress,
        limit: u8,
        cursor: Option<&TonTransactionCursor>,
    ) -> Result<Vec<TonAccountTransaction>, TonRpcError> {
        let mut last_error = None;
        for client in &self.clients {
            match client.account_transactions_page(address, limit, cursor).await {
                Ok(transactions) => return Ok(transactions),
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

    pub async fn message_masterchain_seqno(&self, message_hash: &str) -> Result<Option<u64>, TonRpcError> {
        Ok(self
            .message_outcome(message_hash)
            .await?
            .map(|outcome| outcome.masterchain_seqno))
    }

    pub async fn message_outcome(&self, message_hash: &str) -> Result<Option<TonMessageOutcome>, TonRpcError> {
        let mut last_error = None;
        for client in &self.clients {
            match client.message_outcome(message_hash).await {
                Ok(result) => return Ok(result),
                Err(error) if error.is_retryable() => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(TonRpcError::NoEndpoints))
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
            next_public_request_at: Mutex::new(None),
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

    /// Returns the latest masterchain sequence number reported by TON Center.
    pub async fn current_block(&self) -> Result<u64, TonRpcError> {
        let url = self
            .endpoint
            .join(TONCENTER_V2_MASTERCHAIN_INFO)
            .map_err(|_| TonRpcError::InvalidEndpoint)?;
        let response = self.get(url).await?;
        parse_masterchain_sequence_number(&response)
    }

    pub async fn estimate_fee(&self, request: &TonFeeEstimateRequest) -> Result<TonFeeEstimate, TonRpcError> {
        request
            .address
            .ensure_network(self.network)
            .map_err(|_| TonRpcError::AddressNetwork)?;
        validate_boc(&request.body_boc)?;
        match (&request.init_code_boc, &request.init_data_boc) {
            (Some(code), Some(data)) => {
                validate_boc(code)?;
                validate_boc(data)?;
            },
            (None, None) => {},
            _ => return Err(TonRpcError::InvalidFeeEstimateRequest),
        }

        let mut body = json::json!({
            "address": self.format_address(&request.address)?,
            "body": BASE64.encode(&request.body_boc),
            "ignore_chksig": false,
        });
        if let (Some(code), Some(data)) = (&request.init_code_boc, &request.init_data_boc) {
            body["init_code"] = Json::String(BASE64.encode(code));
            body["init_data"] = Json::String(BASE64.encode(data));
        }
        let body = json::to_vec(&body).map_err(|_| TonRpcError::InvalidResponse)?;
        let url = self
            .endpoint
            .join(TONCENTER_V2_ESTIMATE_FEE)
            .map_err(|_| TonRpcError::InvalidEndpoint)?;
        let response = self.post(url, body).await?;
        parse_fee_estimate(&response)
    }

    /// Returns the newest account transactions first. Pagination and durable
    /// history storage are deliberately owned by the history layer, not this
    /// narrow transport client.
    pub async fn account_transactions(
        &self,
        address: &TonAddress,
        limit: u8,
    ) -> Result<Vec<TonAccountTransaction>, TonRpcError> {
        self.account_transactions_page(address, limit, None).await
    }

    pub async fn account_transactions_page(
        &self,
        address: &TonAddress,
        limit: u8,
        cursor: Option<&TonTransactionCursor>,
    ) -> Result<Vec<TonAccountTransaction>, TonRpcError> {
        if limit == 0 || limit > MAX_TRANSACTION_PAGE_SIZE {
            return Err(TonRpcError::InvalidTransactionQuery);
        }
        let mut url = self
            .endpoint
            .join(TONCENTER_V2_GET_TRANSACTIONS)
            .map_err(|_| TonRpcError::InvalidEndpoint)?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("address", &self.format_address(address)?)
                .append_pair("limit", &limit.to_string());
            if let Some(cursor) = cursor {
                query
                    .append_pair("lt", &cursor.logical_time)
                    .append_pair("hash", &cursor.hash);
            }
        }
        let response = self.get(url).await?;
        parse_account_transactions(&response)
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

    pub async fn message_masterchain_seqno(&self, message_hash: &str) -> Result<Option<u64>, TonRpcError> {
        Ok(self
            .message_outcome(message_hash)
            .await?
            .map(|outcome| outcome.masterchain_seqno))
    }

    pub async fn message_outcome(&self, message_hash: &str) -> Result<Option<TonMessageOutcome>, TonRpcError> {
        if message_hash.is_empty() || message_hash.len() > 256 {
            return Err(TonRpcError::InvalidResponse);
        }
        let mut url = self.endpoint.clone();
        url.set_path(TONCENTER_V3_TRANSACTIONS_BY_MESSAGE);
        url.set_query(None);
        url.query_pairs_mut()
            .append_pair("msg_hash", message_hash)
            .append_pair("direction", "in")
            .append_pair("limit", "1");
        let response = self.get(url).await?;
        parse_message_outcome(&response)
    }

    async fn get(&self, url: Url) -> Result<Vec<u8>, TonRpcError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use http::header::{HeaderName, HeaderValue};
            use mm2_net::transport::slurp_req;

            for attempt in 0..2 {
                self.throttle_public_request().await;
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
                    Ok(Ok((status, _headers, body))) if status.is_success() => return Ok(body),
                    Ok(Ok((status, _headers, _body))) if status.as_u16() == 429 && attempt == 0 => {
                        Timer::sleep(PUBLIC_RATE_LIMIT_RETRY_DELAY.as_secs_f64()).await;
                    },
                    Ok(Ok((status, _headers, _body))) => return Err(TonRpcError::HttpStatus(status.as_u16())),
                    Ok(Err(_)) => return Err(TonRpcError::Transport),
                    Err(_) => return Err(TonRpcError::Timeout),
                }
            }
            Err(TonRpcError::HttpStatus(429))
        }

        #[cfg(target_arch = "wasm32")]
        {
            use mm2_net::wasm::http::FetchRequest;

            for attempt in 0..2 {
                self.throttle_public_request().await;
                let mut request = FetchRequest::get(url.as_str()).cors();
                if let Some(api_key) = self.api_key.as_deref() {
                    request = request.header(TON_API_KEY_HEADER, api_key);
                }

                match Box::pin(request.request_str()).timeout(TON_RPC_TIMEOUT).await {
                    Ok(Ok((status, body))) if status.is_success() => return Ok(body.into_bytes()),
                    Ok(Ok((status, _body))) if status.as_u16() == 429 && attempt == 0 => {
                        Timer::sleep(PUBLIC_RATE_LIMIT_RETRY_DELAY.as_secs_f64()).await;
                    },
                    Ok(Ok((status, _body))) => return Err(TonRpcError::HttpStatus(status.as_u16())),
                    Ok(Err(_)) => return Err(TonRpcError::Transport),
                    Err(_) => return Err(TonRpcError::Timeout),
                }
            }
            Err(TonRpcError::HttpStatus(429))
        }
    }

    async fn post(&self, url: Url, body: Vec<u8>) -> Result<Vec<u8>, TonRpcError> {
        self.throttle_public_request().await;
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

    async fn throttle_public_request(&self) {
        if self.api_key.is_some() {
            return;
        }
        let delay = {
            let mut next_request_at = self.next_public_request_at.lock();
            reserve_public_request_slot(&mut next_request_at, now_ms())
        };
        if !delay.is_zero() {
            Timer::sleep(delay.as_secs_f64()).await;
        }
    }
}

fn reserve_public_request_slot(next_request_at: &mut Option<u64>, now: u64) -> Duration {
    let scheduled_at = match *next_request_at {
        Some(next) if next > now => next,
        Some(_) | None => now,
    };
    *next_request_at = scheduled_at.checked_add(PUBLIC_API_REQUEST_INTERVAL.as_millis() as u64);
    Duration::from_millis(scheduled_at.saturating_sub(now))
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

/// Minimal transaction fields needed for later confirmation and history code.
/// Values remain in native wire units until the domain layer applies accounting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonAccountTransaction {
    pub logical_time: String,
    pub hash: String,
    pub timestamp: u64,
    pub fee: TonAmount,
    pub inbound_message: Option<TonTransactionMessage>,
    pub outbound_messages: Vec<TonTransactionMessage>,
    pub inbound_message_hash: Option<String>,
    pub outbound_message_hashes: Vec<String>,
    /// Serialized account-transaction BOC when the v2 provider returns it.
    /// This is an archival record, not an external BOC that can be rebroadcast.
    pub boc: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonTransactionMessage {
    pub hash: String,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub value: TonAmount,
}

/// The cursor required by TON Center v2 account pagination. The provider
/// requires logical time and transaction hash together, so this type has no
/// partially initialized state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonTransactionCursor {
    pub logical_time: String,
    pub hash: String,
}

impl TonAccountTransaction {
    pub fn cursor(&self) -> TonTransactionCursor {
        TonTransactionCursor {
            logical_time: self.logical_time.clone(),
            hash: self.hash.clone(),
        }
    }
}

/// The BOC components expected by TON Center's `estimateFee` endpoint.
pub struct TonFeeEstimateRequest {
    pub address: TonAddress,
    pub body_boc: Vec<u8>,
    pub init_code_boc: Option<Vec<u8>>,
    pub init_data_boc: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TonFeeComponent {
    pub in_forward_fee: u64,
    pub storage_fee: u64,
    pub gas_fee: u64,
    pub forward_fee: u64,
}

impl TonFeeComponent {
    pub fn total(self) -> Result<u64, TonRpcError> {
        self.in_forward_fee
            .checked_add(self.storage_fee)
            .and_then(|total| total.checked_add(self.gas_fee))
            .and_then(|total| total.checked_add(self.forward_fee))
            .ok_or(TonRpcError::InvalidResponse)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonFeeEstimate {
    pub source: TonFeeComponent,
    pub destinations: Vec<TonFeeComponent>,
}

impl TonFeeEstimate {
    /// Fees charged to the sender's wallet transaction.
    pub fn source_total(&self) -> Result<u64, TonRpcError> {
        self.source.total()
    }

    /// Sum of the source and destination transaction fees reported by the
    /// simulation. This is diagnostic data; a native transfer only debits the
    /// sender by `source_total`.
    pub fn total(&self) -> Result<u64, TonRpcError> {
        self.destinations.iter().try_fold(self.source.total()?, |total, fees| {
            total.checked_add(fees.total()?).ok_or(TonRpcError::InvalidResponse)
        })
    }
}

/// The provider's reference to an externally submitted message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonBroadcastResult {
    pub message_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonMessageOutcome {
    pub masterchain_seqno: u64,
    pub transaction_hash: String,
    pub compute_success: Option<bool>,
    pub action_success: Option<bool>,
    pub recipient_bounced: bool,
    pub outbound_messages: Vec<TonMessageOutcomeMessage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TonMessageOutcomeMessage {
    pub hash: String,
    pub value: TonAmount,
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
    #[display(fmt = "TON fee estimation requires both init code and init data, or neither")]
    InvalidFeeEstimateRequest,
    #[display(fmt = "TON transaction query limit must be between 1 and 100")]
    InvalidTransactionQuery,
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

fn parse_masterchain_sequence_number(bytes: &[u8]) -> Result<u64, TonRpcError> {
    let response: Json = json::from_slice(bytes).map_err(|_| TonRpcError::InvalidResponse)?;
    let result = parse_success_result(&response)?;
    parse_u64(result.get("last").and_then(|last| last.get("seqno")))
}

fn parse_fee_component(value: &Json) -> Result<TonFeeComponent, TonRpcError> {
    Ok(TonFeeComponent {
        in_forward_fee: parse_u64(value.get("in_fwd_fee"))?,
        storage_fee: parse_u64(value.get("storage_fee"))?,
        gas_fee: parse_u64(value.get("gas_fee"))?,
        forward_fee: parse_u64(value.get("fwd_fee"))?,
    })
}

fn parse_fee_estimate(bytes: &[u8]) -> Result<TonFeeEstimate, TonRpcError> {
    let response: Json = json::from_slice(bytes).map_err(|_| TonRpcError::InvalidResponse)?;
    let result = parse_success_result(&response)?;
    let source = parse_fee_component(result.get("source_fees").ok_or(TonRpcError::InvalidResponse)?)?;
    let destinations = result
        .get("destination_fees")
        .and_then(Json::as_array)
        .ok_or(TonRpcError::InvalidResponse)?
        .iter()
        .map(parse_fee_component)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TonFeeEstimate { source, destinations })
}

fn parse_account_transactions(bytes: &[u8]) -> Result<Vec<TonAccountTransaction>, TonRpcError> {
    let response: Json = json::from_slice(bytes).map_err(|_| TonRpcError::InvalidResponse)?;
    let result = parse_success_result(&response)?;
    let transactions = result.as_array().ok_or(TonRpcError::InvalidResponse)?;
    transactions.iter().map(parse_account_transaction).collect()
}

fn parse_account_transaction(value: &Json) -> Result<TonAccountTransaction, TonRpcError> {
    let id = value.get("transaction_id").ok_or(TonRpcError::InvalidResponse)?;
    let logical_time = parse_decimal_string(id.get("lt"))?;
    let hash = parse_hash(id.get("hash"))?;
    let timestamp = parse_u64(value.get("utime"))?;
    let fee = TonAmount::from_nano(parse_u64(value.get("fee"))?);
    let inbound_message = value
        .get("in_msg")
        .filter(|message| !message.is_null())
        .map(parse_transaction_message)
        .transpose()?;
    let outbound_messages = value
        .get("out_msgs")
        .and_then(Json::as_array)
        .ok_or(TonRpcError::InvalidResponse)?
        .iter()
        .map(parse_transaction_message)
        .collect::<Result<Vec<_>, _>>()?;
    let inbound_message_hash = inbound_message.as_ref().map(|message| message.hash.clone());
    let outbound_message_hashes = outbound_messages.iter().map(|message| message.hash.clone()).collect();
    Ok(TonAccountTransaction {
        logical_time,
        hash,
        timestamp,
        fee,
        inbound_message,
        outbound_messages,
        inbound_message_hash,
        outbound_message_hashes,
        boc: parse_optional_boc(value.get("data"))?,
    })
}

fn parse_optional_boc(value: Option<&Json>) -> Result<Option<Vec<u8>>, TonRpcError> {
    match value {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) if !value.is_empty() => {
            let boc = BASE64.decode(value).map_err(|_| TonRpcError::InvalidResponse)?;
            validate_boc(&boc)?;
            Ok(Some(boc))
        },
        _ => Err(TonRpcError::InvalidResponse),
    }
}

fn parse_transaction_message(value: &Json) -> Result<TonTransactionMessage, TonRpcError> {
    Ok(TonTransactionMessage {
        hash: parse_hash(value.get("hash"))?,
        source: parse_optional_address(value.get("source"))?,
        destination: parse_optional_address(value.get("destination"))?,
        value: TonAmount::from_nano(parse_optional_u64(value.get("value"))?),
    })
}

fn parse_optional_address(value: Option<&Json>) -> Result<Option<String>, TonRpcError> {
    match value {
        None | Some(Json::Null) => Ok(None),
        // Toncenter v2 represents the source of an external inbound message as
        // an empty string. It is not an account address, so retain it as the
        // absence of an address instead of rejecting the whole transaction.
        Some(Json::String(address)) if address.trim().is_empty() => Ok(None),
        Some(Json::String(address)) => Ok(Some(address.to_owned())),
        Some(Json::Object(address)) => address
            .get("account_address")
            .and_then(Json::as_str)
            .filter(|address| !address.trim().is_empty())
            .map(str::to_owned)
            .map(Some)
            .ok_or(TonRpcError::InvalidResponse),
        _ => Err(TonRpcError::InvalidResponse),
    }
}

fn parse_optional_u64(value: Option<&Json>) -> Result<u64, TonRpcError> {
    match value {
        None | Some(Json::Null) => Ok(0),
        value => parse_u64(value),
    }
}

fn parse_decimal_string(value: Option<&Json>) -> Result<String, TonRpcError> {
    match value {
        Some(Json::String(value)) if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => {
            Ok(value.to_owned())
        },
        Some(Json::Number(value)) if value.as_u64().is_some() => Ok(value.to_string()),
        _ => Err(TonRpcError::InvalidResponse),
    }
}

fn parse_hash(value: Option<&Json>) -> Result<String, TonRpcError> {
    value
        .and_then(Json::as_str)
        .filter(|hash| !hash.is_empty() && hash.len() <= 256 && hash.trim() == *hash)
        .map(str::to_owned)
        .ok_or(TonRpcError::InvalidResponse)
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

fn parse_message_outcome(bytes: &[u8]) -> Result<Option<TonMessageOutcome>, TonRpcError> {
    let response: Json = json::from_slice(bytes).map_err(|_| TonRpcError::InvalidResponse)?;
    let transactions = response
        .get("transactions")
        .and_then(Json::as_array)
        .ok_or(TonRpcError::InvalidResponse)?;
    match transactions.first() {
        None => Ok(None),
        Some(transaction) => Ok(Some(TonMessageOutcome {
            masterchain_seqno: parse_u64(transaction.get("mc_block_seqno"))?,
            transaction_hash: parse_hash(transaction.get("hash"))?,
            compute_success: transaction
                .pointer("/description/compute_ph/success")
                .and_then(Json::as_bool),
            action_success: transaction
                .pointer("/description/action/success")
                .and_then(Json::as_bool),
            recipient_bounced: transaction
                .get("out_msgs")
                .and_then(Json::as_array)
                .map(|messages| {
                    messages
                        .iter()
                        .any(|message| message.get("bounced").and_then(Json::as_bool) == Some(true))
                })
                .unwrap_or(false)
                || transaction.pointer("/in_msg/bounced").and_then(Json::as_bool) == Some(true),
            outbound_messages: transaction
                .get("out_msgs")
                .and_then(Json::as_array)
                .map(|messages| {
                    messages
                        .iter()
                        .filter_map(|message| {
                            let hash = parse_hash(message.get("hash")).ok()?;
                            let value = parse_optional_u64(message.get("value")).ok()?;
                            Some(TonMessageOutcomeMessage {
                                hash,
                                value: TonAmount::from_nano(value),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })),
    }
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
    fn parses_newest_first_account_transactions_without_losing_wire_identifiers() {
        let transactions = parse_account_transactions(br#"{"ok":true,"result":[{"utime":1700000000,"transaction_id":{"lt":"123","hash":"transaction-hash"},"fee":"42","in_msg":{"hash":"inbound-hash","source":null,"destination":"wallet","value":"0"},"out_msgs":[{"hash":"outbound-hash","source":"wallet","destination":"recipient","value":"100"}]}]}"#).unwrap();

        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].logical_time, "123");
        assert_eq!(transactions[0].fee, TonAmount::from_nano(42));
        assert_eq!(transactions[0].inbound_message_hash.as_deref(), Some("inbound-hash"));
        assert_eq!(transactions[0].outbound_message_hashes, ["outbound-hash"]);
        assert_eq!(transactions[0].outbound_messages[0].value, TonAmount::from_nano(100));
        assert_eq!(transactions[0].boc, None);
        assert_eq!(
            transactions[0].cursor(),
            TonTransactionCursor {
                logical_time: "123".to_owned(),
                hash: "transaction-hash".to_owned(),
            }
        );
    }

    #[test]
    fn accepts_an_external_message_with_an_empty_source_address() {
        let transactions = parse_account_transactions(
            br#"{"ok":true,"result":[{"utime":1700000000,"transaction_id":{"lt":"123","hash":"transaction-hash"},"fee":"42","in_msg":{"hash":"inbound-hash","source":"","destination":"wallet","value":"0"},"out_msgs":[]}]}"#,
        )
        .unwrap();

        assert_eq!(
            transactions[0]
                .inbound_message
                .as_ref()
                .and_then(|message| message.source.as_deref()),
            None
        );
    }

    #[test]
    fn retains_the_optional_serialized_account_transaction() {
        let transactions = parse_account_transactions(
            br#"{"ok":true,"result":[{"utime":1700000000,"transaction_id":{"lt":"123","hash":"transaction-hash"},"fee":"42","data":"AQID","in_msg":null,"out_msgs":[]}]}"#,
        )
        .unwrap();

        assert_eq!(transactions[0].boc, Some(vec![1, 2, 3]));
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
    fn parses_the_masterchain_sequence_number() {
        assert_eq!(
            parse_masterchain_sequence_number(br#"{"ok":true,"result":{"last":{"seqno":12345678}}}"#),
            Ok(12_345_678),
        );
        assert_eq!(
            parse_masterchain_sequence_number(br#"{"ok":true,"result":{"last":{}}}"#),
            Err(TonRpcError::InvalidResponse),
        );
    }

    #[test]
    fn parses_fee_components_without_losing_nano_precision() {
        let estimate = parse_fee_estimate(
            br#"{"ok":true,"result":{"source_fees":{"in_fwd_fee":"1","storage_fee":"2","gas_fee":"3","fwd_fee":"4"},"destination_fees":[{"in_fwd_fee":5,"storage_fee":6,"gas_fee":7,"fwd_fee":8}]}}"#,
        )
        .unwrap();
        assert_eq!(estimate.source.total(), Ok(10));
        assert_eq!(estimate.total(), Ok(36));
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
    fn parses_message_execution_outcome_from_v3() {
        let outcome = parse_message_outcome(
            br#"{"transactions":[{"mc_block_seqno":42,"hash":"transaction-hash","description":{"compute_ph":{"success":true},"action":{"success":true}},"out_msgs":[{"hash":"outbound-hash","value":"100","bounced":false}]}]}"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(outcome.masterchain_seqno, 42);
        assert_eq!(outcome.transaction_hash, "transaction-hash");
        assert_eq!(outcome.compute_success, Some(true));
        assert_eq!(outcome.action_success, Some(true));
        assert!(!outcome.recipient_bounced);
        assert_eq!(
            outcome.outbound_messages,
            vec![TonMessageOutcomeMessage {
                hash: "outbound-hash".to_owned(),
                value: TonAmount::from_nano(100),
            }]
        );
    }

    #[test]
    fn handles_a_message_not_yet_indexed_and_failed_execution() {
        assert_eq!(parse_message_outcome(br#"{"transactions":[]}"#), Ok(None));

        let outcome = parse_message_outcome(
            br#"{"transactions":[{"mc_block_seqno":42,"hash":"transaction-hash","description":{"compute_ph":{"success":false},"action":{"success":false}},"in_msg":{"bounced":true},"out_msgs":[{"bounced":true}]}]}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(outcome.compute_success, Some(false));
        assert_eq!(outcome.action_success, Some(false));
        assert!(outcome.recipient_bounced);
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

    #[test]
    fn reserves_non_overlapping_slots_for_anonymous_requests() {
        let now = 1_000_u64;
        let mut next = None;

        assert_eq!(reserve_public_request_slot(&mut next, now), Duration::ZERO);
        assert_eq!(reserve_public_request_slot(&mut next, now), PUBLIC_API_REQUEST_INTERVAL);
        assert_eq!(
            reserve_public_request_slot(&mut next, now + 500),
            Duration::from_millis(1500),
        );
    }
}
