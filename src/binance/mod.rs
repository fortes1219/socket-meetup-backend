pub mod rest;
pub mod ws;

/// 業務層共用的 Kline view
///
/// REST 跟 WS 各自有自己的 wire-level 結構(`rest::RawKline` tuple、
/// `ws::KlineTick` named object),最後都 normalize 到這個 struct。
/// Phase A-2 落庫時直接從 `Kline` 映射到 `klines` table 的 column。
///
/// 金額欄位(open/high/low/close/volume/quote_volume)維持 `String`,
/// 對齊「給前端的金額一律字串」+ sqlx `NUMERIC` 不丟精度。
#[derive(Debug, Clone)]
pub struct Kline {
  pub symbol: String,
  pub interval: String,
  pub open_time_ms: i64,
  pub close_time_ms: i64,
  pub open: String,
  pub high: String,
  pub low: String,
  pub close: String,
  pub volume: String,
  /// Wire 有但 V002 schema 沒,保留 wire 完整性,落庫時 drop。
  #[allow(dead_code)]
  pub quote_volume: String,
  pub trades_count: i64,
}
