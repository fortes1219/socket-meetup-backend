use std::str::FromStr;

use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;

use crate::binance::Kline;

/// DB row(對應 `klines` table 一列)。
///
/// 跟 `binance::Kline`(wire String)/ `api::klines::KlineDto`(API String)分層:
///   wire(Binance)→ Kline(String)→ KlineRow(BigDecimal)→ KlineDto(String)
/// 每層責任清楚,convert 在邊界做。
#[derive(Debug)]
pub struct KlineRow {
  /// Filter 條件已固定 symbol/interval,batch 內每筆都一樣 → DTO 不重複輸出,
  /// 但保留完整列(schema-first)。
  #[allow(dead_code)]
  pub symbol: String,
  #[allow(dead_code)]
  pub interval: String,
  pub open_time: DateTime<Utc>,
  pub close_time: DateTime<Utc>,
  pub open: BigDecimal,
  pub high: BigDecimal,
  pub low: BigDecimal,
  pub close: BigDecimal,
  pub volume: BigDecimal,
  pub trades_count: i32,
}

/// 落庫:`INSERT ... ON CONFLICT DO UPDATE`,容忍 WS 重連 replay。
///
/// `quote_volume` wire 有但 V002 schema 沒(刻意 keep schema 輕量),drop 不寫。
pub async fn upsert(pool: &PgPool, k: &Kline) -> Result<()> {
  let open = BigDecimal::from_str(&k.open).context("open not numeric")?;
  let high = BigDecimal::from_str(&k.high).context("high not numeric")?;
  let low = BigDecimal::from_str(&k.low).context("low not numeric")?;
  let close = BigDecimal::from_str(&k.close).context("close not numeric")?;
  let volume = BigDecimal::from_str(&k.volume).context("volume not numeric")?;
  let open_time = Utc
    .timestamp_millis_opt(k.open_time_ms)
    .single()
    .context("invalid open_time_ms")?;
  let close_time = Utc
    .timestamp_millis_opt(k.close_time_ms)
    .single()
    .context("invalid close_time_ms")?;
  let trades_count =
    i32::try_from(k.trades_count).context("trades_count overflow i32")?;

  sqlx::query!(
    r#"
    INSERT INTO klines (
      symbol, "interval", open_time, close_time,
      open, high, low, close, volume, trades_count
    )
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
    ON CONFLICT (symbol, "interval", open_time) DO UPDATE SET
      close_time   = EXCLUDED.close_time,
      open         = EXCLUDED.open,
      high         = EXCLUDED.high,
      low          = EXCLUDED.low,
      close        = EXCLUDED.close,
      volume       = EXCLUDED.volume,
      trades_count = EXCLUDED.trades_count
    "#,
    k.symbol,
    k.interval,
    open_time,
    close_time,
    open,
    high,
    low,
    close,
    volume,
    trades_count,
  )
  .execute(pool)
  .await
  .context("klines upsert failed")?;

  Ok(())
}

/// 撈最近 `limit` 筆(getBars `type:'init'`)。
pub async fn fetch_init(
  pool: &PgPool,
  symbol: &str,
  interval: &str,
  limit: i64,
) -> Result<Vec<KlineRow>> {
  sqlx::query_as!(
    KlineRow,
    r#"
    SELECT
      symbol,
      "interval",
      open_time,
      close_time,
      open,
      high,
      low,
      close,
      volume,
      trades_count
    FROM klines
    WHERE symbol = $1 AND "interval" = $2
    ORDER BY open_time DESC
    LIMIT $3
    "#,
    symbol,
    interval,
    limit,
  )
  .fetch_all(pool)
  .await
  .context("klines fetch_init failed")
}

/// 撈 `end_time` **之前**(exclusive)最近 `limit` 筆(getBars `type:'forward'`,
/// 往左拖載入更早歷史)。
pub async fn fetch_forward(
  pool: &PgPool,
  symbol: &str,
  interval: &str,
  end_time_ms: i64,
  limit: i64,
) -> Result<Vec<KlineRow>> {
  let end_time = Utc
    .timestamp_millis_opt(end_time_ms)
    .single()
    .context("invalid endTime ms")?;

  sqlx::query_as!(
    KlineRow,
    r#"
    SELECT
      symbol,
      "interval",
      open_time,
      close_time,
      open,
      high,
      low,
      close,
      volume,
      trades_count
    FROM klines
    WHERE symbol = $1 AND "interval" = $2 AND open_time < $3
    ORDER BY open_time DESC
    LIMIT $4
    "#,
    symbol,
    interval,
    end_time,
    limit,
  )
  .fetch_all(pool)
  .await
  .context("klines fetch_forward failed")
}
