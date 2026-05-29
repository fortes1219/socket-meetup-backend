use axum::{
  Json,
  http::StatusCode,
  response::{IntoResponse, Response},
};
use serde::Serialize;

/// 統一 error body(§6.6 REST API 規格,前端 Zod / ts-rest 正式依據)。
///
/// - `error`:前端穩定 key(固定 enum code 字串),前端據此分流處理
/// - `message`:固定可控的人類可讀文字
///
/// **絕不**把 serde / sqlx / upstream 的原始錯誤序列化給 client;原始 error chain 只進 log。
#[derive(Debug, Serialize)]
pub struct ErrorBody {
  pub error: &'static str,
  pub message: &'static str,
}

/// Handler / middleware / extractor 共用 error type。
///
/// 透過 `IntoResponse` map 成 §6.6 的 `(status, ErrorBody)`。多數 `?` 來源
/// (sqlx / reqwest / anyhow …)經 blanket `From` 收斂成 `Internal`,完整 chain
/// 進 log,對外只回固定 `internal_error`。
///
/// Mutation domain error(`not_found` / `conflict` / `symbol_not_found` /
/// `upstream_error` / `empty_patch` / `committed_broadcast_failed`)在 A-3.3b
/// 寫路徑再擴充,本 enum 目前只含 read path + admin 共用所需的 variant。
pub enum AppError {
  /// 401:`X-Admin-Token` 缺漏或不符(§6.6 Auth / §8 C)
  Unauthorized,
  /// 400:strict input rejection(query / path / json parse 失敗)
  InvalidParam,
  /// 500:manual `/admin/broadcast` emit 失敗,或 `/` namespace 不存在(§6.6)
  BroadcastFailed,
  /// 500:未預期內部錯誤(DB failure 等);原始錯誤只進 log,不外洩
  Internal(anyhow::Error),
}

impl IntoResponse for AppError {
  fn into_response(self) -> Response {
    let (status, body) = match self {
      AppError::Unauthorized => (
        StatusCode::UNAUTHORIZED,
        ErrorBody {
          error: "unauthorized",
          message: "invalid admin token",
        },
      ),
      AppError::InvalidParam => (
        StatusCode::BAD_REQUEST,
        ErrorBody {
          error: "invalid_param",
          message: "invalid request parameter",
        },
      ),
      AppError::BroadcastFailed => (
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorBody {
          error: "broadcast_failed",
          message: "failed to broadcast callUpdate",
        },
      ),
      AppError::Internal(e) => {
        tracing::error!(error = ?e, "handler error");
        (
          StatusCode::INTERNAL_SERVER_ERROR,
          ErrorBody {
            error: "internal_error",
            message: "internal server error",
          },
        )
      }
    };
    (status, Json(body)).into_response()
  }
}

/// 任何 `Into<anyhow::Error>`(含 `?` 的多數 case)收斂成 `Internal`。
/// `AppError` 本身未實作 `std::error::Error`,故不與 std reflexive `From<T> for T` 衝突。
impl<E> From<E> for AppError
where
  E: Into<anyhow::Error>,
{
  fn from(err: E) -> Self {
    Self::Internal(err.into())
  }
}
