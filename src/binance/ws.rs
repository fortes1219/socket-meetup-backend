use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use socketioxide::SocketIo;
use sqlx::PgPool;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use super::Kline;
use crate::db;

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

/// 連 `{base_ws}/ws/{symbol}@kline_{interval}`,持續收 tick。
///
/// **Emit 規則(§6.5 KlineTickPayload):**
/// - **每筆 tick(closed AND unclosed)**都 emit `kline` event 到 `/quote`,nested camelCase
///   payload,`closed` 旗標攜帶,**room-filtered** 到 `${symbol.uppercase()}:${interval}`
///   (client 用 `subscribe` event 加入,見 `socket::register_namespaces`)
/// - 上游目前 hardcoded **單一 `BTCUSDT:1m`**,其他 symbol/interval 的 room 不會有 tick
///   流入(已知限制,見 handoff `/quote` sub-phase 段)
///
/// **DB 落庫(§6.5 不變):** 只 `is_closed = true` 才 upsert klines。
/// unclosed tick:DEBUG 印 close 價避免洗版。
///
/// 不 throttle unclosed tick;reconnect / 動態 fan-in 上游留後續 phase。
pub async fn subscribe_kline_stream(
  pool: PgPool,
  io: SocketIo,
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

          // 1. 落庫只在 closed(§6.5 + Phase A-2),unclosed 印 DEBUG 不洗版
          if is_closed {
            info!(
              symbol = %kline.symbol,
              interval = %kline.interval,
              open = %kline.open,
              close = %kline.close,
              volume = %kline.volume,
              "CLOSED kline"
            );
            if let Err(e) = db::klines::upsert(&pool, &kline).await {
              tracing::error!(?e, symbol = %kline.symbol, "klines upsert failed");
            }
          } else {
            debug!(close = %kline.close, "tick");
          }

          // 2. emit `kline` 給 room — closed AND unclosed 都 emit(§6.5 KlineTickPayload)
          //    room key:`${SYMBOL_UPPER}:${interval}`,frontend 必須先 subscribe 加入 room
          //    金額已是 String(api-money-as-string memory rule);openTime camelCase
          if let Some(ns) = io.of("/quote") {
            let room = format!("{}:{}", kline.symbol.to_uppercase(), kline.interval);
            let payload = json!({
              "symbol": kline.symbol,
              "interval": kline.interval,
              "kline": {
                "openTime": kline.open_time_ms,
                "open": kline.open,
                "high": kline.high,
                "low": kline.low,
                "close": kline.close,
                "volume": kline.volume,
                "closed": is_closed,
              },
            });
            if let Err(e) = ns.to(room).emit("kline", &payload).await {
              warn!(?e, "emit kline failed");
            }
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
