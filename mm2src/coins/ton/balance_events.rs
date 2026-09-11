use super::{TonCoin, TON_DECIMALS};
use crate::{utxo::utxo_common::big_decimal_from_sat_unsigned, MarketCoinOps};
use async_trait::async_trait;
use common::executor::Timer;
use futures::channel::oneshot;
use mm2_event_stream::{Broadcaster, Event, EventStreamer, NoDataIn, StreamHandlerInput, StreamerId};
use serde::Deserialize;
use serde_json::{json, Value as Json};

const DEFAULT_STREAM_INTERVAL_SECONDS: f64 = 15.0;
const MIN_STREAM_INTERVAL_SECONDS: f64 = 1.0;
const MAX_STREAM_INTERVAL_SECONDS: f64 = 3_600.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
struct TonBalanceStreamingConfig {
    stream_interval_seconds: f64,
}

impl Default for TonBalanceStreamingConfig {
    fn default() -> Self {
        TonBalanceStreamingConfig {
            stream_interval_seconds: DEFAULT_STREAM_INTERVAL_SECONDS,
        }
    }
}

/// Polls the one supported TON wallet and sends an initial balance followed by
/// changes. The streaming manager shares one instance across all subscribers.
pub struct TonBalanceEventStreamer {
    coin: TonCoin,
    interval_seconds: f64,
}

impl TonBalanceEventStreamer {
    pub fn try_new(config: Option<Json>, coin: TonCoin) -> Result<Self, String> {
        let config: TonBalanceStreamingConfig = config
            .map(serde_json::from_value)
            .unwrap_or(Ok(Default::default()))
            .map_err(|error| error.to_string())?;
        if !config.stream_interval_seconds.is_finite()
            || !(MIN_STREAM_INTERVAL_SECONDS..=MAX_STREAM_INTERVAL_SECONDS).contains(&config.stream_interval_seconds)
        {
            return Err(format!(
                "TON balance stream_interval_seconds must be between {MIN_STREAM_INTERVAL_SECONDS} and {MAX_STREAM_INTERVAL_SECONDS}"
            ));
        }
        Ok(TonBalanceEventStreamer {
            coin,
            interval_seconds: config.stream_interval_seconds,
        })
    }
}

#[async_trait]
impl EventStreamer for TonBalanceEventStreamer {
    type DataInType = NoDataIn;

    fn streamer_id(&self) -> StreamerId {
        StreamerId::Balance {
            coin: self.coin.ticker().to_owned(),
        }
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        _: impl StreamHandlerInput<NoDataIn>,
    ) {
        let _ = ready_tx.send(Ok(()));
        let streamer_id = self.streamer_id();
        let coin = self.coin;
        let address = match coin.my_address() {
            Ok(address) => address,
            Err(error) => {
                broadcaster.broadcast(Event::err(streamer_id, json!({ "error": error.to_string() })));
                return;
            },
        };
        let mut last_balance = None;

        loop {
            match coin.wallet_information().await {
                Ok(wallet) if last_balance != Some(wallet.balance) => {
                    let balance = wallet.balance;
                    last_balance = Some(balance);
                    broadcaster.broadcast(Event::new(
                        streamer_id.clone(),
                        json!([{
                            "ticker": coin.ticker(),
                            "address": address,
                            "balance": {
                                "spendable": big_decimal_from_sat_unsigned(balance.as_nano(), TON_DECIMALS),
                                "unspendable": 0,
                            },
                        }]),
                    ));
                },
                Ok(_) => {},
                Err(error) => broadcaster.broadcast(Event::err(
                    streamer_id.clone(),
                    json!({
                        "ticker": coin.ticker(),
                        "address": address,
                        "error": error.to_string(),
                    }),
                )),
            }
            Timer::sleep(self.interval_seconds).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TonBalanceStreamingConfig, DEFAULT_STREAM_INTERVAL_SECONDS};

    #[test]
    fn uses_a_rate_limit_safe_default_interval() {
        assert_eq!(
            TonBalanceStreamingConfig::default().stream_interval_seconds,
            DEFAULT_STREAM_INTERVAL_SECONDS
        );
    }
}
