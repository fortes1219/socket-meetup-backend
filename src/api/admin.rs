use axum::{
  extract::{Request, State},
  http::{HeaderMap, StatusCode},
  middleware::Next,
  response::Response,
};
use serde_json::json;

use crate::AppState;

/// `/admin/*` 共用 middleware:檢查 `X-Admin-Token` header 對齊 env `ADMIN_TOKEN`(§8 C)。
///
/// 掛在 `/admin` sub-router 的 `route_layer`,所有 admin route 都先過這關;
/// A-3.3 trading-pairs CRUD route 加進同一 sub-router 自動沿用,不重複寫驗證。
pub async fn require_admin_token(
  State(state): State<AppState>,
  headers: HeaderMap,
  req: Request,
  next: Next,
) -> Result<Response, StatusCode> {
  let provided = headers.get("X-Admin-Token").and_then(|v| v.to_str().ok());
  match provided {
    Some(token) if token == state.admin_token => Ok(next.run(req).await),
    _ => Err(StatusCode::UNAUTHORIZED),
  }
}

/// `POST /admin/broadcast`(§9 step 17 / §6.6 demo 武器)
///
/// 不動資料,純廣播 `callUpdate` 到 `/` namespace 所有 client(§6.5 fan-out:無過濾)。
/// payload 固定 `{ resource: "trading-pairs", timestamp }`(§6.5 CallUpdatePayload)。
pub async fn broadcast(State(state): State<AppState>) -> StatusCode {
  if let Some(ns) = state.io.of("/") {
    let res = ns
      .emit(
        "callUpdate",
        &json!({
          "resource": "trading-pairs",
          "timestamp": chrono::Utc::now().timestamp_millis(),
        }),
      )
      .await;
    if let Err(e) = res {
      tracing::warn!(?e, "emit callUpdate failed");
      return StatusCode::INTERNAL_SERVER_ERROR;
    }
  }
  tracing::info!("admin broadcast callUpdate(resource=trading-pairs)");
  StatusCode::NO_CONTENT
}
