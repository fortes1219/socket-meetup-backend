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

/// `GET /admin/audit/recent` 的 row(A-3.3c)。
///
/// `symbol` 由 INNER JOIN trading_pairs by id 取得 —— audit FK 保證 pair 存在,
/// **不過濾 `deleted_at IS NULL`**:否則 DELETE 之後 `removed` audit 會跟著消失,
/// 看不出歷史紀錄當初指的是哪個 symbol。
#[derive(Debug)]
pub struct AuditEntryRow {
  pub audit_id: Uuid,
  pub trading_pair_id: Uuid,
  pub symbol: String,
  pub action: String,
  pub changed_by: String,
  pub occurred_at: DateTime<Utc>,
}

/// 最近 `limit` 筆 audit,排序 `occurred_at DESC, audit_id DESC`。
///
/// `audit_id` 為 UUIDv7(時序單調),當同 transaction 多筆 audit 撞同一 `occurred_at`
/// 時做穩定 tie-breaker —— A-3.3b 已實證 PATCH 兩欄改的 `disabled`/`reordered` 會撞。
/// handler 已保證 `1 <= limit <= 200`(>200 / <=0 → 400 invalid_param,不 clamp)。
pub async fn list_recent_audit(pool: &PgPool, limit: i64) -> Result<Vec<AuditEntryRow>> {
  sqlx::query_as!(
    AuditEntryRow,
    r#"
    SELECT
      a.audit_id,
      a.trading_pair_id,
      p.symbol AS "symbol!",
      a.action,
      a.changed_by,
      a.occurred_at
    FROM trading_pair_audit a
    INNER JOIN trading_pairs p ON p.id = a.trading_pair_id
    ORDER BY a.occurred_at DESC, a.audit_id DESC
    LIMIT $1
    "#,
    limit,
  )
  .fetch_all(pool)
  .await
  .context("trading_pairs list_recent_audit failed")
}

// ─── 寫路徑(A-3.3b)───
//
// service 層的 typed domain error(§6.6 invariant #8:handler 不准直接 SQL)。
// 刻意 **不** impl `Into<AppError>`:`AppError` 的 blanket `From<E: Into<anyhow::Error>>`
// 會把 `Conflict`/`NotFound` 誤收斂成 500 internal_error;handler 用明確 match 轉換。
#[derive(Debug, thiserror::Error)]
pub enum TradingPairError {
  /// `:id` 不存在或已 soft-deleted → 404
  #[error("trading pair not found")]
  NotFound,
  /// symbol UNIQUE 違反(含 soft-deleted 佔用)→ 409
  #[error("symbol already exists")]
  Conflict,
  /// 其他 DB 失敗(commit 前)→ 500 internal_error;原始錯誤只進 log
  #[error(transparent)]
  Db(#[from] sqlx::Error),
}

/// PATCH 可變欄位(None = 該欄未送)。
pub struct PairPatch {
  pub enabled: Option<bool>,
  pub display_order: Option<i32>,
}

/// PATCH 結果:`Changed` 才需要 emit callUpdate;`NoOp` 不 emit、不改 updated_at。
pub enum PatchOutcome {
  Changed(AdminPairRow),
  NoOp(AdminPairRow),
}

/// POST:tx 內 INSERT(UUIDv7,enabled=false / display_order=0 由 DB default)+ audit `added`。
/// symbol UNIQUE 違反 → `Conflict`。emit 由 handler 在 commit 後做(guardrail #3)。
pub async fn insert_with_audit(
  pool: &PgPool,
  id: Uuid,
  symbol: &str,
  base_asset: &str,
  quote_asset: &str,
) -> Result<AdminPairRow, TradingPairError> {
  let mut tx = pool.begin().await?;

  let inserted = sqlx::query_as!(
    AdminPairRow,
    r#"
    INSERT INTO trading_pairs (id, symbol, base_asset, quote_asset)
    VALUES ($1, $2, $3, $4)
    RETURNING id, symbol, base_asset, quote_asset, enabled, display_order, created_at, updated_at
    "#,
    id,
    symbol,
    base_asset,
    quote_asset,
  )
  .fetch_one(&mut *tx)
  .await;

  let row = match inserted {
    Ok(r) => r,
    Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
      return Err(TradingPairError::Conflict);
    }
    Err(e) => return Err(TradingPairError::Db(e)),
  };

  sqlx::query!(
    r#"
    INSERT INTO trading_pair_audit (audit_id, trading_pair_id, action, changed_by)
    VALUES ($1, $2, 'added', 'admin:demo')
    "#,
    Uuid::now_v7(),
    id,
  )
  .execute(&mut *tx)
  .await?;

  tx.commit().await?;
  Ok(row)
}

/// PATCH:SELECT FOR UPDATE(!deleted,無 → `NotFound`)→ 比對實際變更。
/// no-op → `NoOp`(不寫、不改 updated_at,tx drop 即 rollback);
/// 有變更 → UPDATE(updated_at=now)+ 每個變更欄位一筆 audit + commit。
pub async fn update_with_audit(
  pool: &PgPool,
  id: Uuid,
  patch: PairPatch,
) -> Result<PatchOutcome, TradingPairError> {
  let mut tx = pool.begin().await?;

  let current = sqlx::query_as!(
    AdminPairRow,
    r#"
    SELECT id, symbol, base_asset, quote_asset, enabled, display_order, created_at, updated_at
    FROM trading_pairs
    WHERE id = $1 AND deleted_at IS NULL
    FOR UPDATE
    "#,
    id,
  )
  .fetch_optional(&mut *tx)
  .await?
  .ok_or(TradingPairError::NotFound)?;

  // 只保留「與現值不同」的欄位 = 實際變更
  let enabled_change = patch.enabled.filter(|&v| v != current.enabled);
  let order_change = patch.display_order.filter(|&v| v != current.display_order);

  if enabled_change.is_none() && order_change.is_none() {
    return Ok(PatchOutcome::NoOp(current));
  }

  let new_enabled = enabled_change.unwrap_or(current.enabled);
  let new_order = order_change.unwrap_or(current.display_order);

  let row = sqlx::query_as!(
    AdminPairRow,
    r#"
    UPDATE trading_pairs
    SET enabled = $2, display_order = $3, updated_at = now()
    WHERE id = $1
    RETURNING id, symbol, base_asset, quote_asset, enabled, display_order, created_at, updated_at
    "#,
    id,
    new_enabled,
    new_order,
  )
  .fetch_one(&mut *tx)
  .await?;

  // 兩欄都變 → 同 tx 寫兩筆 audit(§6.6 PATCH 語意)
  if let Some(v) = enabled_change {
    let action = if v { "enabled" } else { "disabled" };
    sqlx::query!(
      r#"INSERT INTO trading_pair_audit (audit_id, trading_pair_id, action, changed_by)
         VALUES ($1, $2, $3, 'admin:demo')"#,
      Uuid::now_v7(),
      id,
      action,
    )
    .execute(&mut *tx)
    .await?;
  }
  if order_change.is_some() {
    sqlx::query!(
      r#"INSERT INTO trading_pair_audit (audit_id, trading_pair_id, action, changed_by)
         VALUES ($1, $2, 'reordered', 'admin:demo')"#,
      Uuid::now_v7(),
      id,
    )
    .execute(&mut *tx)
    .await?;
  }

  tx.commit().await?;
  Ok(PatchOutcome::Changed(row))
}

/// DELETE:SELECT FOR UPDATE(!deleted,無 → `NotFound`)→ soft delete(deleted_at/updated_at=now)
/// + audit `removed` + commit。回 204 無 body,故不需 RETURNING。
pub async fn soft_delete_with_audit(pool: &PgPool, id: Uuid) -> Result<(), TradingPairError> {
  let mut tx = pool.begin().await?;

  let exists = sqlx::query_scalar!(
    r#"SELECT id FROM trading_pairs WHERE id = $1 AND deleted_at IS NULL FOR UPDATE"#,
    id,
  )
  .fetch_optional(&mut *tx)
  .await?;

  if exists.is_none() {
    return Err(TradingPairError::NotFound);
  }

  sqlx::query!(
    r#"UPDATE trading_pairs SET deleted_at = now(), updated_at = now() WHERE id = $1"#,
    id,
  )
  .execute(&mut *tx)
  .await?;

  sqlx::query!(
    r#"INSERT INTO trading_pair_audit (audit_id, trading_pair_id, action, changed_by)
       VALUES ($1, $2, 'removed', 'admin:demo')"#,
    Uuid::now_v7(),
    id,
  )
  .execute(&mut *tx)
  .await?;

  tx.commit().await?;
  Ok(())
}
