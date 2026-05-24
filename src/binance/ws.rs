use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use super::Kline;

/// 外層 envelope(`e/E/s/k`),我們只關心 `k`。
#[derive(Debug, Deserialize)]
struct KlineEvent {
  #[serde(rename = "k")]
  kline: KlineTick,
}

/// `k` 物件內欄位用單字母 key,`#[serde(rename)]` 對應到語意 field。
#[derive(Debug, Deserialize)]
struct KlineTick {
  #[serde(rename = "t")]
  open_time_ms: i64,
  #[serde(rename = "T")]
  close_time_ms: i64,
  #[serde(rename = "s")]
  symbol: String,
  #[serde(rename = "i")]
  interval: String,
  #[serde(rename = "o")]
  open: String,
  #[serde(rename = "h")]
  high: String,
  #[serde(rename = "l")]
  low: String,
  #[serde(rename = "c")]
  close: String,
  #[serde(rename = "v")]
  volume: String,
  #[serde(rename = "q")]
  quote_volume: String,
  #[serde(rename = "n")]
  trades_count: i64,
  #[serde(rename = "x")]
  is_closed: bool,
}

impl From<KlineTick> for Kline {
  fn from(t: KlineTick) -> Self {
    Kline {
      symbol: t.symbol,
      interval: t.interval,
      open_time_ms: t.open_time_ms,
      close_time_ms: t.close_time_ms,
      open: t.open,
      high: t.high,
      low: t.low,
      close: t.close,
      volume: t.volume,
      quote_volume: t.quote_volume,
      trades_count: t.trades_count,
    }
  }
}

/// 連 `{base_ws}/ws/{symbol}@kline_{interval}`,持續 log 收到的 tick。
///
/// - `is_closed=false` → DEBUG(避免每秒洗版)
/// - `is_closed=true`  → INFO  CLOSED kline(每 `interval` 一次,真正落庫的對象)
///
/// Phase A-1 階段不 reconnect / 不落庫,純驗證 stream 通。
pub async fn subscribe_kline_stream(
  base_ws: &str,
  symbol: &str,
  interval: &str,
) -> Result<()> {
  let url = format!(
    "{}/ws/{}@kline_{}",
    base_ws,
    symbol.to_lowercase(),
    interval
  );
  info!(%url, "connecting binance ws");

  let (ws_stream, _) = connect_async(&url)
    .await
    .context("binance ws connect failed")?;
  info!("ws connected");

  let (_write, mut read) = ws_stream.split();
  while let Some(msg) = read.next().await {
    match msg.context("ws read failed")? {
      Message::Text(txt) => match serde_json::from_str::<KlineEvent>(txt.as_str()) {
        Ok(evt) => {
          let is_closed = evt.kline.is_closed;
          let kline: Kline = evt.kline.into();
          if is_closed {
            info!(
              symbol = %kline.symbol,
              interval = %kline.interval,
              open = %kline.open,
              close = %kline.close,
              volume = %kline.volume,
              "CLOSED kline"
            );
          } else {
            debug!(close = %kline.close, "tick");
          }
        }
        Err(e) => warn!(?e, raw = %txt, "ws parse failed"),
      },
      Message::Ping(_) => { /* tokio-tungstenite auto-pongs */ }
      Message::Close(frame) => {
        info!(?frame, "ws closed by server");
        break;
      }
      _ => {}
    }
  }
  Ok(())
}
