use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::{
  api::extractors::StrictQuery,
  db::{
    self,
    trading_pairs::{AdminPairRow, PublicPairRow},
  },
  error::AppError,
};

/// `GET /api/v1/trading-pairs` response item(§6.6 `PublicTradingPair`)。
///
/// **刻意不暴露** id / enabled / created_at / updated_at(public 面)。
/// 欄位 snake_case(對齊 klines DTO)。
#[derive(Debug, Serialize)]
pub struct PublicTradingPair {
  pub symbol: String,
  pub base_asset: String,
  pub quote_asset: String,
  pub display_order: i32,
}

impl From<PublicPairRow> for PublicTradingPair {
  fn from(r: PublicPairRow) -> Self {
    Self {
      symbol: r.symbol,
      base_asset: r.base_asset,
      quote_asset: r.quote_asset,
      display_order: r.display_order,
    }
  }
}

/// `GET /admin/trading-pairs` response item(§6.6 `AdminTradingPair`)。
///
/// id 為 uuid string;timestamp 一律 integer ms epoch(非金額,不適用 string 規則)。
#[derive(Debug, Serialize)]
pub struct AdminTradingPair {
  pub id: String,
  pub symbol: String,
  pub base_asset: String,
  pub quote_asset: String,
  pub enabled: bool,
  pub display_order: i32,
  pub created_at: i64,
  pub updated_at: i64,
}

impl From<AdminPairRow> for AdminTradingPair {
  fn from(r: AdminPairRow) -> Self {
    Self {
      id: r.id.to_string(),
      symbol: r.symbol,
      base_asset: r.base_asset,
      quote_asset: r.quote_asset,
      enabled: r.enabled,
      display_order: r.display_order,
      created_at: r.created_at.timestamp_millis(),
      updated_at: r.updated_at.timestamp_millis(),
    }
  }
}

/// `GET /admin/trading-pairs` query。
///
/// `include_disabled` 省略 = `true`(§6.6:POST 新增預設 disabled,後台預設要看得到);
/// 非 bool(如 `?include_disabled=foo`)→ `StrictQuery` 轉 400 invalid_param。
/// `deny_unknown_fields`:未知 query field(如 `?foo=bar`)→ 400 invalid_param(§6.6 strict validation)。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminListQuery {
  #[serde(default = "default_include_disabled")]
  pub include_disabled: bool,
}

fn default_include_disabled() -> bool {
  true
}

/// `GET /api/v1/trading-pairs` — enabled && !deleted,`display_order ASC, symbol ASC`。
pub async fn list_public(
  State(pool): State<PgPool>,
) -> Result<Json<Vec<PublicTradingPair>>, AppError> {
  let rows = db::trading_pairs::list_public(&pool).await?;
  Ok(Json(
    rows.into_iter().map(PublicTradingPair::from).collect(),
  ))
}

/// `GET /admin/trading-pairs?include_disabled=true|false` — !deleted;false 再篩 enabled。
pub async fn list_admin(
  State(pool): State<PgPool>,
  StrictQuery(q): StrictQuery<AdminListQuery>,
) -> Result<Json<Vec<AdminTradingPair>>, AppError> {
  let rows = db::trading_pairs::list_admin(&pool, q.include_disabled).await?;
  Ok(Json(rows.into_iter().map(AdminTradingPair::from).collect()))
}
