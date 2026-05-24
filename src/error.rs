use axum::{
  Json,
  http::StatusCode,
  response::{IntoResponse, Response},
};
use serde_json::json;

/// Handler 共用 error type。
///
/// 任何實現 `Into<anyhow::Error>` 的 error(含 `?` 運算子的多數 case)都會
/// auto-convert,log 完整 error chain 後對 client 回 500 + `{"error":"internal"}`。
/// Phase A-3 之後用 `thiserror` 切 domain error(401 / 404 / 422 …)。
pub struct AppError(pub anyhow::Error);

impl IntoResponse for AppError {
  fn into_response(self) -> Response {
    tracing::error!(error = ?self.0, "handler error");
    (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(json!({ "error": "internal" })),
    )
      .into_response()
  }
}

impl<E> From<E> for AppError
where
  E: Into<anyhow::Error>,
{
  fn from(err: E) -> Self {
    Self(err.into())
  }
}
