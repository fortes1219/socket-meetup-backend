use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// `GET /api/v1/trading-pairs` 的 row(public 暴露面)。
///
/// 刻意只取 4 欄:不含 id / enabled / timestamps —— 對齊 §6.6 `PublicTradingPair`
/// 的暴露決策(投影點即此 SELECT,不 over-fetch)。
#[derive(Debug)]
pub struct PublicPairRow {
  pub symbol: String,
  pub base_asset: String,
  pub quote_asset: String,
  pub display_order: i32,
}

/// `GET /admin/trading-pairs` 的 row(admin 全欄)。
///
/// DB-native 型別;轉 ms epoch / uuid string 在 api 層 `From` 邊界做(對齊 klines 分層)。
#[derive(Debug)]
pub struct AdminPairRow {
  pub id: Uuid,
  pub symbol: String,
  pub base_asset: String,
  pub quote_asset: String,
  pub enabled: bool,
  pub display_order: i32,
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}

/// 前端可見清單:`enabled && !deleted`,固定排序 `display_order ASC, symbol ASC`。
/// 命中 partial index `idx_pairs_visible`。
pub async fn list_public(pool: &PgPool) -> Result<Vec<PublicPairRow>> {
  sqlx::query_as!(
    PublicPairRow,
    r#"
    SELECT symbol, base_asset, quote_asset, display_order
    FROM trading_pairs
    WHERE deleted_at IS NULL AND enabled = true
    ORDER BY display_order ASC, symbol ASC
    "#,
  )
  .fetch_all(pool)
  .await
  .context("trading_pairs list_public failed")
}

/// 後台清單:`!deleted`;`include_disabled = false` 時再篩 `enabled`。
///
/// `(enabled OR $1)`:`$1 = true` → 全部未刪除;`$1 = false` → 只 enabled。
pub async fn list_admin(pool: &PgPool, include_disabled: bool) -> Result<Vec<AdminPairRow>> {
  sqlx::query_as!(
    AdminPairRow,
    r#"
    SELECT id, symbol, base_asset, quote_asset, enabled, display_order, created_at, updated_at
    FROM trading_pairs
    WHERE deleted_at IS NULL AND (enabled OR $1)
    ORDER BY display_order ASC, symbol ASC
    "#,
    include_disabled,
  )
  .fetch_all(pool)
  .await
  .context("trading_pairs list_admin failed")
}
