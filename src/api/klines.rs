use axum::{
  Json,
  extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::{
  db::{self, klines::KlineRow},
  error::AppError,
};

#[derive(Debug, Deserialize)]
pub struct KlinesQuery {
  pub symbol: String,
  pub interval: String,
  #[serde(default = "default_limit")]
  pub limit: i64,
  /// ms epoch;`Some` = forward mode(撈 endTime 之前 limit 筆,往左拖),
  /// `None` = init mode(撈最近 limit 筆)。`backward` 不實作(§9 step 13)。
  #[serde(rename = "endTime")]
  pub end_time: Option<i64>,
}

fn default_limit() -> i64 {
  500
}

/// API DTO:給前端的 K 線。
///
/// 金額(open/high/low/close/volume)**全 string**(對齊 api-money-as-string 規則)。
/// 時間用 ms epoch(對齊 klinecharts v10 `getBars` 期望的 `timestamp` 格式)。
#[derive(Debug, Serialize)]
pub struct KlineDto {
  pub open_time: i64,
  pub close_time: i64,
  pub open: String,
  pub high: String,
  pub low: String,
  pub close: String,
  pub volume: String,
  pub trades_count: i32,
}

impl From<KlineRow> for KlineDto {
  fn from(r: KlineRow) -> Self {
    // BigDecimal::to_string() 預設修剪末尾 0(76907.00000000 → "76907"),
    // 對前端不友善;強制 with_scale(8) 對齊 PG NUMERIC(20,8) schema,
    // 所有 row 一致 8 位小數,前端不用自己 pad。
    KlineDto {
      open_time: r.open_time.timestamp_millis(),
      close_time: r.close_time.timestamp_millis(),
      open: r.open.with_scale(8).to_string(),
      high: r.high.with_scale(8).to_string(),
      low: r.low.with_scale(8).to_string(),
      close: r.close.with_scale(8).to_string(),
      volume: r.volume.with_scale(8).to_string(),
      trades_count: r.trades_count,
    }
  }
}

pub async fn get_klines(
  State(pool): State<PgPool>,
  Query(q): Query<KlinesQuery>,
) -> Result<Json<Vec<KlineDto>>, AppError> {
  let rows = match q.end_time {
    None => db::klines::fetch_init(&pool, &q.symbol, &q.interval, q.limit).await?,
    Some(ts) => {
      db::klines::fetch_forward(&pool, &q.symbol, &q.interval, ts, q.limit).await?
    }
  };
  Ok(Json(rows.into_iter().map(KlineDto::from).collect()))
}
