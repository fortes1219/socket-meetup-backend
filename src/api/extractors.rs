use axum::{
  Json,
  extract::{FromRequest, FromRequestParts, Path, Query, Request},
  http::request::Parts,
};
use serde::de::DeserializeOwned;

use crate::error::AppError;

/// Strict query extractor。
///
/// 包一層 Axum `Query`,parse 失敗(缺欄、型別錯誤、無法 deserialize)統一轉成
/// `AppError::InvalidParam` → 400 `{ error: "invalid_param", ... }`(JSON),
/// 不讓 Axum 回預設 plain-text rejection(§6.6 Validation)。
///
/// A-3.3a read path 只用到 query rejection;A-3.3b 寫路徑會再補 path / json 的
/// strict 版本,共用同一 `invalid_param` 收斂規則。
pub struct StrictQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for StrictQuery<T>
where
  T: DeserializeOwned,
  S: Send + Sync,
{
  type Rejection = AppError;

  async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
    match Query::<T>::from_request_parts(parts, state).await {
      Ok(Query(value)) => Ok(Self(value)),
      Err(rej) => {
        // 原始 rejection 只進 log,不回給 client(固定 message)。
        tracing::debug!(?rej, "query rejection → invalid_param");
        Err(AppError::InvalidParam)
      }
    }
  }
}

/// Strict path extractor(A-3.3b)。
///
/// path param 解析失敗(如 `:id` 非合法 UUID)→ 400 `invalid_param`(§6.6 Validation)。
pub struct StrictPath<T>(pub T);

impl<T, S> FromRequestParts<S> for StrictPath<T>
where
  T: DeserializeOwned + Send,
  S: Send + Sync,
{
  type Rejection = AppError;

  async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
    match Path::<T>::from_request_parts(parts, state).await {
      Ok(Path(value)) => Ok(Self(value)),
      Err(rej) => {
        tracing::debug!(?rej, "path rejection → invalid_param");
        Err(AppError::InvalidParam)
      }
    }
  }
}

/// Strict JSON body extractor(A-3.3b)。
///
/// body 解析失敗(invalid JSON / `deny_unknown_fields` 未知欄位 / scalar 型別錯 /
/// missing field)→ 400 `invalid_param`。注意:`empty_patch`(合法 JSON 但欄位全 None)
/// **不在這裡**判,由 handler 比對 None 後回 400 `empty_patch`。
///
/// 消耗 request body → 為 `FromRequest`,在 handler 參數中必須放最後一個。
pub struct StrictJson<T>(pub T);

impl<T, S> FromRequest<S> for StrictJson<T>
where
  T: DeserializeOwned,
  S: Send + Sync,
{
  type Rejection = AppError;

  async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
    match Json::<T>::from_request(req, state).await {
      Ok(Json(value)) => Ok(Self(value)),
      Err(rej) => {
        tracing::debug!(?rej, "json rejection → invalid_param");
        Err(AppError::InvalidParam)
      }
    }
  }
}
