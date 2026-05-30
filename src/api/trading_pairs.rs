use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
  AppState,
  api::extractors::{StrictJson, StrictPath, StrictQuery},
  binance::rest::{self, ExchangeInfoError},
  db::{
    self,
    trading_pairs::{
      AdminPairRow, AuditEntryRow, PairPatch, PatchOutcome, PublicPairRow, TradingPairError,
    },
  },
  error::AppError,
  socket,
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

// ─── 寫路徑(A-3.3b)───

/// `POST /admin/trading-pairs` body。client 只給 symbol,base/quote 由幣安給。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePairBody {
  pub symbol: String,
}

/// `PATCH /admin/trading-pairs/{id}` body。兩欄皆 optional;全 None → empty_patch。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchPairBody {
  pub enabled: Option<bool>,
  pub display_order: Option<i32>,
}

/// service 層 typed error → HTTP error 的明確映射(不走 blanket From,見 [`TradingPairError`])。
fn map_pair_err(e: TradingPairError) -> AppError {
  match e {
    TradingPairError::NotFound => AppError::NotFound,
    TradingPairError::Conflict => AppError::Conflict,
    TradingPairError::Db(err) => AppError::Internal(err.into()),
  }
}

/// `POST /admin/trading-pairs` — 201 AdminTradingPair。
///
/// 流程(§6.6):symbol trim+upper(空→400)→ **tx 外** 問幣安(422/502)→ tx INSERT+audit
/// (UNIQUE→409)→ commit → emit(失敗→committed_broadcast_failed)。
pub async fn create(
  State(state): State<AppState>,
  StrictJson(body): StrictJson<CreatePairBody>,
) -> Result<(StatusCode, Json<AdminTradingPair>), AppError> {
  let symbol = body.symbol.trim().to_uppercase();
  if symbol.is_empty() {
    return Err(AppError::InvalidParam);
  }

  // tx 外:問幣安取 authoritative base/quote(不信 client 亂塞)
  let meta = rest::fetch_exchange_info(&state.http, &state.binance_rest_base, &symbol)
    .await
    .map_err(|e| match e {
      ExchangeInfoError::SymbolNotFound => AppError::SymbolNotFound,
      ExchangeInfoError::Upstream(err) => {
        tracing::warn!(error = ?err, "binance exchangeInfo upstream error");
        AppError::UpstreamError
      }
    })?;

  let id = Uuid::now_v7();
  let row = db::trading_pairs::insert_with_audit(
    &state.pool,
    id,
    &meta.symbol,
    &meta.base_asset,
    &meta.quote_asset,
  )
  .await
  .map_err(map_pair_err)?;

  socket::emit_call_update(&state.io)
    .await
    .map_err(|_| AppError::CommittedBroadcastFailed)?;

  Ok((StatusCode::CREATED, Json(AdminTradingPair::from(row))))
}

/// `PATCH /admin/trading-pairs/{id}` — 200 AdminTradingPair。
///
/// 空 body→400 empty_patch;display_order<0→400 invalid_param;`:id` 不存在→404。
/// 實際變更才 emit;no-op→200 不 emit、不改 updated_at(§6.6 PATCH 語意)。
pub async fn update(
  State(state): State<AppState>,
  StrictPath(id): StrictPath<Uuid>,
  StrictJson(body): StrictJson<PatchPairBody>,
) -> Result<Json<AdminTradingPair>, AppError> {
  if body.enabled.is_none() && body.display_order.is_none() {
    return Err(AppError::EmptyPatch);
  }
  if matches!(body.display_order, Some(v) if v < 0) {
    return Err(AppError::InvalidParam);
  }

  let patch = PairPatch {
    enabled: body.enabled,
    display_order: body.display_order,
  };
  let outcome = db::trading_pairs::update_with_audit(&state.pool, id, patch)
    .await
    .map_err(map_pair_err)?;

  let row = match outcome {
    PatchOutcome::Changed(row) => {
      socket::emit_call_update(&state.io)
        .await
        .map_err(|_| AppError::CommittedBroadcastFailed)?;
      row
    }
    PatchOutcome::NoOp(row) => row,
  };

  Ok(Json(AdminTradingPair::from(row)))
}

/// `DELETE /admin/trading-pairs/{id}` — 204。soft delete + audit `removed`,commit 後 emit。
pub async fn delete(
  State(state): State<AppState>,
  StrictPath(id): StrictPath<Uuid>,
) -> Result<StatusCode, AppError> {
  db::trading_pairs::soft_delete_with_audit(&state.pool, id)
    .await
    .map_err(map_pair_err)?;

  socket::emit_call_update(&state.io)
    .await
    .map_err(|_| AppError::CommittedBroadcastFailed)?;

  Ok(StatusCode::NO_CONTENT)
}

// ─── audit recent(A-3.3c)───

const DEFAULT_AUDIT_LIMIT: i64 = 50;
const MAX_AUDIT_LIMIT: i64 = 200;

/// `GET /admin/audit/recent` query。
///
/// `limit` 省略 = 50;`<=0` 或 `>200` → 400 invalid_param(**不 clamp**)。
/// 非 integer / 未知欄位 → `StrictQuery` 收斂成 400 invalid_param。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentAuditQuery {
  pub limit: Option<i64>,
}

/// `GET /admin/audit/recent` response item(§6.6 `AuditEntry`)。
///
/// `action` 維持 String —— 值空間 `added`/`enabled`/`disabled`/`removed`/`reordered`
/// 由 migration 0003 的 `chk_trading_pair_audit_action` CHECK 在 DB 層保證,
/// 前端 Zod literal-union 收斂(避免 backend 為 strong enum 擴 scope)。
#[derive(Debug, Serialize)]
pub struct AuditEntry {
  pub audit_id: String,
  pub trading_pair_id: String,
  pub symbol: String,
  pub action: String,
  pub changed_by: String,
  pub occurred_at: i64,
}

impl From<AuditEntryRow> for AuditEntry {
  fn from(r: AuditEntryRow) -> Self {
    Self {
      audit_id: r.audit_id.to_string(),
      trading_pair_id: r.trading_pair_id.to_string(),
      symbol: r.symbol,
      action: r.action,
      changed_by: r.changed_by,
      occurred_at: r.occurred_at.timestamp_millis(),
    }
  }
}

/// `GET /admin/audit/recent?limit` — 200 `AuditEntry[]`,排序 `occurred_at DESC, audit_id DESC`。
///
/// 純 read(無 emit / 無 mutation)。soft-deleted pair 的 audit 仍會回(§6.6 + guardrail #1)。
pub async fn list_audit_recent(
  State(pool): State<PgPool>,
  StrictQuery(q): StrictQuery<RecentAuditQuery>,
) -> Result<Json<Vec<AuditEntry>>, AppError> {
  let limit = q.limit.unwrap_or(DEFAULT_AUDIT_LIMIT);
  if !(1..=MAX_AUDIT_LIMIT).contains(&limit) {
    return Err(AppError::InvalidParam);
  }
  let rows = db::trading_pairs::list_recent_audit(&pool, limit).await?;
  Ok(Json(rows.into_iter().map(AuditEntry::from).collect()))
}
