use axum::{
  extract::{Request, State},
  http::{HeaderMap, StatusCode},
  middleware::Next,
  response::Response,
};
use serde_json::json;

use crate::{AppState, error::AppError};

/// `/admin/*` 共用 middleware:檢查 `X-Admin-Token` header 對齊 env `ADMIN_TOKEN`(§8 C)。
///
/// 掛在 `/admin` sub-router 的 `route_layer`,所有 admin route 都先過這關;
/// trading-pairs / audit route 加進同一 sub-router 自動沿用,不重複寫驗證。
/// fail → 401 JSON `{ error: "unauthorized", message: "invalid admin token" }`(§6.6 Auth)。
pub async fn require_admin_token(
  State(state): State<AppState>,
  headers: HeaderMap,
  req: Request,
  next: Next,
) -> Result<Response, AppError> {
  let provided = headers.get("X-Admin-Token").and_then(|v| v.to_str().ok());
  match provided {
    Some(token) if token == state.admin_token => Ok(next.run(req).await),
    _ => Err(AppError::Unauthorized),
  }
}

/// `POST /admin/broadcast`(§9 step 17 / §6.6 demo 武器)
///
/// 不動資料,純廣播 `callUpdate` 到 `/` namespace 所有 client(§6.5 fan-out:無過濾)。
/// payload 固定 `{ resource: "trading-pairs", timestamp }`(§6.5 CallUpdatePayload)。
///
/// §6.6 error 語意:無 DB mutation,emit 失敗 **或** `/` namespace 不存在 → 500
/// `broadcast_failed`(不得回 204);成功 → 204。
pub async fn broadcast(State(state): State<AppState>) -> Result<StatusCode, AppError> {
  // `/` namespace 不存在也算 broadcast failure(§6.6:不得 silently 回 204)。
  let ns = state.io.of("/").ok_or_else(|| {
    tracing::warn!("broadcast: / namespace not registered");
    AppError::BroadcastFailed
  })?;

  ns.emit(
    "callUpdate",
    &json!({
      "resource": "trading-pairs",
      "timestamp": chrono::Utc::now().timestamp_millis(),
    }),
  )
  .await
  .map_err(|e| {
    tracing::warn!(?e, "emit callUpdate failed");
    AppError::BroadcastFailed
  })?;

  tracing::info!("admin broadcast callUpdate(resource=trading-pairs)");
  Ok(StatusCode::NO_CONTENT)
}
