use crate::context::CoinsActivationContext;
use crate::prelude::*;
use crate::standalone_coin::{
    InitStandaloneCoinActivationOps, InitStandaloneCoinError, InitStandaloneCoinInitialStatus,
    InitStandaloneCoinTaskHandleShared, InitStandaloneCoinTaskManagerShared,
};
use async_trait::async_trait;
use coins::coin_balance::{CoinBalanceReport, IguanaWalletBalance};
use coins::coin_errors::MyAddressError;
use coins::my_tx_history_v2::TxHistoryStorage;
use coins::ton::{TonActivationRequest, TonCoin, TonCoinConfig, TonProtocolInfo};
use coins::tx_history_storage::CreateTxHistoryStorageError;
use coins::{BalanceError, CoinBalance, CoinProtocol, MarketCoinOps, MmCoin, PrivKeyBuildPolicy, RegisterCoinError};
use common::executor::SpawnFuture;
use crypto::hw_rpc_task::{HwRpcTaskAwaitingStatus, HwRpcTaskUserAction};
use crypto::CryptoCtxError;
use derive_more::Display;
use futures::compat::Future01CompatExt;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_event_stream::StreamingManager;
use mm2_metrics::MetricsArc;
use mm2_number::BigDecimal;
use rpc_task::RpcTaskError;
use ser_error_derive::SerializeErrorType;
use serde_derive::Serialize;
use serde_json::Value as Json;
use std::collections::HashMap;
use std::time::Duration;

pub type TonCoinTaskManagerShared = InitStandaloneCoinTaskManagerShared<TonCoin>;
pub type TonCoinRpcTaskHandleShared = InitStandaloneCoinTaskHandleShared<TonCoin>;
pub type TonCoinAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type TonCoinUserAction = HwRpcTaskUserAction;

#[derive(Clone, Serialize)]
pub struct TonCoinActivationResult {
    pub ticker: String,
    pub address: String,
    pub current_block: u64,
    pub wallet_balance: CoinBalanceReport<CoinBalance>,
}

impl CurrentBlock for TonCoinActivationResult {
    fn current_block(&self) -> u64 {
        self.current_block
    }
}

impl GetAddressesBalances for TonCoinActivationResult {
    fn get_addresses_balances(&self) -> HashMap<String, BigDecimal> {
        self.wallet_balance
            .to_addresses_total_balances(&self.ticker)
            .into_iter()
            .map(|(address, balance)| (address, balance.unwrap_or_default()))
            .collect()
    }
}

#[derive(Clone, Serialize)]
#[non_exhaustive]
pub enum TonCoinInProgressStatus {
    ActivatingCoin,
    RequestingWalletState,
    RequestingWalletBalance,
    Finishing,
}

impl InitStandaloneCoinInitialStatus for TonCoinInProgressStatus {
    fn initial_status() -> Self {
        TonCoinInProgressStatus::ActivatingCoin
    }
}

#[derive(Clone, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
#[non_exhaustive]
pub enum TonCoinInitError {
    #[display(fmt = "Error on coin {ticker} creation: {error}")]
    CoinCreationError {
        ticker: String,
        error: String,
    },
    CoinIsAlreadyActivated {
        ticker: String,
    },
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut {
        duration: Duration,
    },
    CouldNotGetBalance(String),
    CouldNotGetBlockCount(String),
    Internal(String),
}

impl From<BalanceError> for TonCoinInitError {
    fn from(error: BalanceError) -> Self {
        TonCoinInitError::CouldNotGetBalance(error.to_string())
    }
}

impl From<RegisterCoinError> for TonCoinInitError {
    fn from(error: RegisterCoinError) -> Self {
        match error {
            RegisterCoinError::CoinIsInitializedAlready { coin } => {
                TonCoinInitError::CoinIsAlreadyActivated { ticker: coin }
            },
            RegisterCoinError::Internal(error) => TonCoinInitError::Internal(error),
        }
    }
}

impl From<RpcTaskError> for TonCoinInitError {
    fn from(error: RpcTaskError) -> Self {
        match error {
            RpcTaskError::Timeout(duration) => TonCoinInitError::TaskTimedOut { duration },
            error => TonCoinInitError::Internal(error.to_string()),
        }
    }
}

impl From<CryptoCtxError> for TonCoinInitError {
    fn from(error: CryptoCtxError) -> Self {
        TonCoinInitError::Internal(error.to_string())
    }
}

impl From<CreateTxHistoryStorageError> for TonCoinInitError {
    fn from(error: CreateTxHistoryStorageError) -> Self {
        match error {
            CreateTxHistoryStorageError::Internal(error) => TonCoinInitError::Internal(error),
        }
    }
}

impl From<MyAddressError> for TonCoinInitError {
    fn from(error: MyAddressError) -> Self {
        TonCoinInitError::Internal(error.to_string())
    }
}

impl From<TonCoinInitError> for InitStandaloneCoinError {
    fn from(error: TonCoinInitError) -> Self {
        match error {
            TonCoinInitError::CoinCreationError { ticker, error } => {
                InitStandaloneCoinError::CoinCreationError { ticker, error }
            },
            TonCoinInitError::CoinIsAlreadyActivated { ticker } => {
                InitStandaloneCoinError::CoinIsAlreadyActivated { ticker }
            },
            TonCoinInitError::TaskTimedOut { duration } => InitStandaloneCoinError::TaskTimedOut { duration },
            TonCoinInitError::CouldNotGetBalance(error) | TonCoinInitError::CouldNotGetBlockCount(error) => {
                InitStandaloneCoinError::Transport(error)
            },
            TonCoinInitError::Internal(error) => InitStandaloneCoinError::Internal(error),
        }
    }
}

impl TxHistory for TonActivationRequest {
    fn tx_history(&self) -> bool {
        self.tx_history
    }
}

impl TryFromCoinProtocol for TonProtocolInfo {
    fn try_from_coin_protocol(protocol: CoinProtocol) -> Result<Self, MmError<CoinProtocol>> {
        match protocol {
            CoinProtocol::TON(protocol) => Ok(protocol),
            protocol => MmError::err(protocol),
        }
    }
}

#[async_trait]
impl InitStandaloneCoinActivationOps for TonCoin {
    type ActivationRequest = TonActivationRequest;
    type StandaloneProtocol = TonProtocolInfo;
    type ActivationResult = TonCoinActivationResult;
    type ActivationError = TonCoinInitError;
    type InProgressStatus = TonCoinInProgressStatus;
    type AwaitingStatus = TonCoinAwaitingStatus;
    type UserAction = TonCoinUserAction;

    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &TonCoinTaskManagerShared {
        &activation_ctx.init_ton_coin_task_manager
    }

    async fn init_standalone_coin(
        ctx: MmArc,
        ticker: String,
        coin_conf: Json,
        activation_request: &TonActivationRequest,
        _protocol_info: TonProtocolInfo,
        task_handle: TonCoinRpcTaskHandleShared,
    ) -> MmResult<Self, TonCoinInitError> {
        task_handle
            .update_in_progress_status(TonCoinInProgressStatus::RequestingWalletState)
            .map_mm_err()?;
        let key_policy = PrivKeyBuildPolicy::detect_priv_key_policy(&ctx).map_mm_err()?;
        let config = TonCoinConfig::from_json(coin_conf).map_to_mm(|error| TonCoinInitError::CoinCreationError {
            ticker: ticker.clone(),
            error: error.to_string(),
        })?;
        TonCoin::activate_with_context(&ctx, config, activation_request.clone(), key_policy)
            .await
            .map_to_mm(|error| TonCoinInitError::CoinCreationError {
                ticker,
                error: error.to_string(),
            })
    }

    async fn get_activation_result(
        &self,
        _ctx: MmArc,
        task_handle: TonCoinRpcTaskHandleShared,
        _activation_request: &TonActivationRequest,
    ) -> MmResult<TonCoinActivationResult, TonCoinInitError> {
        task_handle
            .update_in_progress_status(TonCoinInProgressStatus::RequestingWalletBalance)
            .map_mm_err()?;
        let current_block = self
            .current_block_number()
            .await
            .map_to_mm(|error| TonCoinInitError::CouldNotGetBlockCount(error.to_string()))?;
        let balance = self.my_balance().compat().await.map_mm_err()?;
        let address = self.my_address().map_mm_err()?;

        Ok(TonCoinActivationResult {
            ticker: self.ticker().to_owned(),
            address: address.clone(),
            current_block,
            wallet_balance: CoinBalanceReport::Iguana(IguanaWalletBalance { address, balance }),
        })
    }

    fn start_history_background_fetching(
        &self,
        _metrics: MetricsArc,
        storage: impl TxHistoryStorage,
        streaming_manager: StreamingManager,
        _current_balances: HashMap<String, BigDecimal>,
    ) {
        let coin = self.clone();
        self.spawner()
            .spawn(async move { coin.history_loop(storage, Some(streaming_manager)).await });
    }
}
