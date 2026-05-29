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
/// 透過 `IntoResponse` map 成 §6.6 的 `(status, ErrorBody)`。非預期的 `?` 來源
/// (sqlx / reqwest / anyhow …)經 blanket `From` 收斂成 `Internal`,完整 chain
/// 進 log,對外只回固定 `internal_error`。
///
/// Domain error(conflict / not_found / symbol_not_found …)**不走** blanket `From`,
/// 由 service 層的 typed error 在 handler 明確 map 成對應 variant —— 否則
/// 例如 UNIQUE violation 會被 blanket `From` 誤收斂成 500 internal_error。
pub enum AppError {
  /// 401:`X-Admin-Token` 缺漏或不符(§6.6 Auth / §8 C)
  Unauthorized,
  /// 400:strict input rejection(query / path / json parse 失敗)
  InvalidParam,
  /// 400:PATCH body 全為 None(§6.6 PATCH 語意)
  EmptyPatch,
  /// 404:`:id` 不存在或已 soft-deleted(§6.6)
  NotFound,
  /// 409:symbol UNIQUE 違反(含 soft-deleted 佔用,§6.6 POST)
  Conflict,
  /// 422:binance exchangeInfo 查無此 symbol(空 symbols / code -1121)
  SymbolNotFound,
  /// 502:binance upstream 失敗(network / timeout / non-2xx / 解析失敗)
  UpstreamError,
  /// 500:manual `/admin/broadcast` emit 失敗,或 `/` namespace 不存在(§6.6)
  BroadcastFailed,
  /// 500:mutation **已 commit**,只 callUpdate emit 失敗 → 前端 refetch、不得 retry(§6.6 三態)
  CommittedBroadcastFailed,
  /// 500:未預期內部錯誤(commit 前的 DB failure 等);原始錯誤只進 log,不外洩
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
      AppError::EmptyPatch => (
        StatusCode::BAD_REQUEST,
        ErrorBody {
          error: "empty_patch",
          message: "patch body must contain at least one field",
        },
      ),
      AppError::NotFound => (
        StatusCode::NOT_FOUND,
        ErrorBody {
          error: "not_found",
          message: "trading pair not found",
        },
      ),
      AppError::Conflict => (
        StatusCode::CONFLICT,
        ErrorBody {
          error: "conflict",
          message: "symbol already exists",
        },
      ),
      AppError::SymbolNotFound => (
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorBody {
          error: "symbol_not_found",
          message: "symbol not found on binance",
        },
      ),
      AppError::UpstreamError => (
        StatusCode::BAD_GATEWAY,
        ErrorBody {
          error: "upstream_error",
          message: "binance upstream request failed",
        },
      ),
      AppError::BroadcastFailed => (
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorBody {
          error: "broadcast_failed",
          message: "failed to broadcast callUpdate",
        },
      ),
      AppError::CommittedBroadcastFailed => (
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorBody {
          error: "committed_broadcast_failed",
          message: "change committed but broadcast failed; refetch instead of retrying",
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
