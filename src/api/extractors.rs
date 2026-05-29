use axum::{
  extract::{FromRequestParts, Query},
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
