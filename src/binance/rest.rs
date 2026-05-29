use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

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

/// binance `exchangeInfo` 取回的 symbol metadata(camelCase wire)。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawExchangeInfo {
  symbols: Vec<RawSymbol>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSymbol {
  symbol: String,
  base_asset: String,
  quote_asset: String,
}

/// 幣安錯誤 envelope(invalid-symbol = HTTP 400 + `{ "code": -1121 }`)。
#[derive(Debug, Deserialize)]
struct BinanceErrorBody {
  code: i64,
}

/// binance 認可的 symbol metadata(POST 用,base/quote 由幣安給,不信 client)。
pub struct SymbolMeta {
  pub symbol: String,
  pub base_asset: String,
  pub quote_asset: String,
}

/// `fetch_exchange_info` 的兩種失敗(§6.6 POST 流程):
/// - `SymbolNotFound` → 422(空 symbols 或 code -1121)
/// - `Upstream` → 502(network / timeout / non-2xx / 解析失敗);原始錯誤只進 log
pub enum ExchangeInfoError {
  SymbolNotFound,
  Upstream(anyhow::Error),
}

/// 問幣安 `GET {base}/api/v3/exchangeInfo?symbol=`,取 base/quote asset。
///
/// timeout 由傳入的 `Client`(main.rs 建構時設定)保證 —— 否則「timeout → 502」
/// 只是紙上規格。symbol-not-found 與其他 upstream 失敗嚴格分流(§6.6)。
pub async fn fetch_exchange_info(
  client: &Client,
  base: &str,
  symbol: &str,
) -> std::result::Result<SymbolMeta, ExchangeInfoError> {
  let url = format!("{}/api/v3/exchangeInfo?symbol={}", base, symbol);

  let resp = client.get(&url).send().await.map_err(|e| {
    ExchangeInfoError::Upstream(
      anyhow::Error::new(e).context("binance exchangeInfo request failed"),
    )
  })?;

  let status = resp.status();
  let body = resp.text().await.map_err(|e| {
    ExchangeInfoError::Upstream(
      anyhow::Error::new(e).context("binance exchangeInfo body read failed"),
    )
  })?;

  if status.is_success() {
    let info: RawExchangeInfo = serde_json::from_str(&body).map_err(|e| {
      ExchangeInfoError::Upstream(
        anyhow::Error::new(e).context("binance exchangeInfo not valid JSON"),
      )
    })?;
    match info.symbols.into_iter().next() {
      Some(s) => Ok(SymbolMeta {
        symbol: s.symbol,
        base_asset: s.base_asset,
        quote_asset: s.quote_asset,
      }),
      None => Err(ExchangeInfoError::SymbolNotFound),
    }
  } else if status == reqwest::StatusCode::BAD_REQUEST
    && matches!(serde_json::from_str::<BinanceErrorBody>(&body), Ok(e) if e.code == -1121)
  {
    Err(ExchangeInfoError::SymbolNotFound)
  } else {
    Err(ExchangeInfoError::Upstream(anyhow::anyhow!(
      "binance exchangeInfo unexpected status: {status}"
    )))
  }
}
