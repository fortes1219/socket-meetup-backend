use std::time::{Duration, Instant};

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

/// 連 `{base_ws}/ws/{symbol}@kline_{interval}` 的**單次**連線生命週期,持續收 tick
/// 到 server close(`Ok(())`)或 stream 出錯(`Err`)為止。
///
/// **單次嘗試** —— reconnect 由 [`run_with_reconnect`] 包,caller 通常呼叫 wrapper、
/// 不直接呼叫此 fn。
///
/// **Emit 規則(§6.5 KlineTickPayload):**
/// - **每筆 tick(closed AND unclosed)**都 emit `kline` event 到 `/quote`,nested camelCase
///   payload,`closed` 旗標攜帶,**room-filtered** 到 `${symbol.uppercase()}:${interval}`
///   (client 用 `subscribe` event 加入,見 `socket::register_namespaces`)
/// - 上游覆蓋的 (symbol, interval) 集合由 `main` 的 `BINANCE_KLINE_SYMBOLS` allowlist 決定;
///   allowlist 外的 room 不會有 tick(graceful,frontend 仍可 subscribe)
///
/// **DB 落庫(§6.5 不變):** 只 `is_closed = true` 才 upsert klines。
/// unclosed tick:DEBUG 印 close 價避免洗版。
///
/// 不 throttle unclosed tick(留後續 phase)。
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

/// Per-symbol reconnect loop:長期跑(隨 tokio runtime 結束),包住 [`subscribe_kline_stream`]
/// 的單次連線生命週期。
///
/// **Backoff:** 起跳 5s,每次失敗倍增,上限 60s。**Stable-reset:** 上次 stream 撐
/// 超過 60s 後失敗(視為穩定連線後的真實 disconnect),backoff 重設回 5s,不會卡在
/// 60s 永遠不變。
///
/// **Per-symbol 隔離:** 此函數絕不 panic / 絕不 return —— 單一 symbol 失敗只 log,
/// **不影響其他 symbol 的 task**(在 `main` 端各自 `tokio::spawn`)。
///
/// Server 主動關連線(`Message::Close`)→ `subscribe_kline_stream` 回 `Ok(())`,
/// wrapper 也照 backoff 重連(視為 normal lifecycle endpoint,不是 error)。
pub async fn run_with_reconnect(
  pool: PgPool,
  io: SocketIo,
  base_ws: String,
  symbol: String,
  interval: String,
) {
  const INITIAL_BACKOFF: Duration = Duration::from_secs(5);
  const MAX_BACKOFF: Duration = Duration::from_secs(60);
  let mut backoff = INITIAL_BACKOFF;

  loop {
    let attempt_start = Instant::now();
    // 每輪 clone:PgPool / SocketIo 內部都 Arc,clone 成本 negligible
    let outcome =
      subscribe_kline_stream(pool.clone(), io.clone(), &base_ws, &symbol, &interval).await;
    let lasted = attempt_start.elapsed();

    match outcome {
      Ok(()) => warn!(
        symbol = %symbol,
        interval = %interval,
        lasted_secs = lasted.as_secs(),
        "WS stream returned Ok (likely server close); will reconnect"
      ),
      Err(e) => tracing::error!(
        ?e,
        symbol = %symbol,
        interval = %interval,
        lasted_secs = lasted.as_secs(),
        "WS stream errored; will reconnect"
      ),
    }

    // Stable-reset:上次撐超過 MAX_BACKOFF 視為穩定連線後失敗,backoff 從 INITIAL 重來
    if lasted >= MAX_BACKOFF {
      backoff = INITIAL_BACKOFF;
    }

    info!(
      symbol = %symbol,
      interval = %interval,
      backoff_secs = backoff.as_secs(),
      "WS reconnect: sleeping before next attempt"
    );
    tokio::time::sleep(backoff).await;
    backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
  }
}
