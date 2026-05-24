use anyhow::{Context, Result};
use reqwest::Client;

use super::Kline;

/// Binance REST `/api/v3/klines` 的 raw response 是 12-tuple array:
/// [open_time, open, high, low, close, volume, close_time, quote_vol,
///  trades, taker_buy_base, taker_buy_quote, ignore]
///
/// serde 對 array-of-array 預設 deserialize 成 `Vec<Tuple>`,直接用 type alias。
type RawKline = (
  i64,    // 0: open_time_ms
  String, // 1: open
  String, // 2: high
  String, // 3: low
  String, // 4: close
  String, // 5: volume
  i64,    // 6: close_time_ms
  String, // 7: quote_asset_volume
  i64,    // 8: number_of_trades
  String, // 9: taker_buy_base_asset_volume
  String, // 10: taker_buy_quote_asset_volume
  String, // 11: ignore
);

/// 撈最近 `limit` 筆 K 線。
///
/// Binance API:
///   GET {base}/api/v3/klines?symbol=BTCUSDT&interval=1m&limit=500
///
/// 之後 Phase A-2 加 `endTime` 參數實作 getBars 的 forward(往左拖載入更早歷史)。
pub async fn fetch_klines(
  client: &Client,
  base: &str,
  symbol: &str,
  interval: &str,
  limit: u32,
) -> Result<Vec<Kline>> {
  let url = format!(
    "{}/api/v3/klines?symbol={}&interval={}&limit={}",
    base, symbol, interval, limit
  );

  let raws: Vec<RawKline> = client
    .get(&url)
    .send()
    .await
    .context("binance REST request failed")?
    .error_for_status()
    .context("binance REST returned non-2xx")?
    .json()
    .await
    .context("binance REST response not valid JSON")?;

  Ok(
    raws
      .into_iter()
      .map(|r| Kline {
        symbol: symbol.to_string(),
        interval: interval.to_string(),
        open_time_ms: r.0,
        open: r.1,
        high: r.2,
        low: r.3,
        close: r.4,
        volume: r.5,
        close_time_ms: r.6,
        quote_volume: r.7,
        trades_count: r.8,
      })
      .collect(),
  )
}
