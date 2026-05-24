use std::net::SocketAddr;

use anyhow::Result;
use axum::{Json, Router, routing::get};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod api;
mod binance;
mod db;
mod error;

#[tokio::main]
async fn main() -> Result<()> {
  // Load .env(本機 dev;production env 由外部注入,沒 .env 不 panic)
  dotenvy::dotenv().ok();

  // Tracing(讀 RUST_LOG,fallback info + sqlx warn)
  tracing_subscriber::fmt()
    .with_env_filter(
      EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
    )
    .init();

  // Postgres pool(啟動時即時連接,連不上 fail fast)
  let database_url = std::env::var("DATABASE_URL")
    .map_err(|_| anyhow::anyhow!("DATABASE_URL not set; check .env"))?;
  let pool = PgPoolOptions::new()
    .max_connections(5)
    .connect(&database_url)
    .await?;
  tracing::info!("connected to postgres");

  // ─── Phase A-1/A-2: Binance REST + WS ingestion ───
  let rest_base = std::env::var("BINANCE_REST_BASE")
    .map_err(|_| anyhow::anyhow!("BINANCE_REST_BASE not set; check .env"))?;
  let ws_base = std::env::var("BINANCE_WS_BASE")
    .map_err(|_| anyhow::anyhow!("BINANCE_WS_BASE not set; check .env"))?;

  // Task A: REST 撈 500 筆當 backfill,落庫(一次性)
  let rest_pool = pool.clone();
  tokio::spawn(async move {
    let client = reqwest::Client::new();
    match binance::rest::fetch_klines(&client, &rest_base, "BTCUSDT", "1m", 500).await {
      Ok(klines) => {
        tracing::info!(
          count = klines.len(),
          last_close = %klines.last().map(|k| k.close.as_str()).unwrap_or("(empty)"),
          "REST fetched"
        );
        let mut ok = 0usize;
        let mut err = 0usize;
        for k in &klines {
          match db::klines::upsert(&rest_pool, k).await {
            Ok(_) => ok += 1,
            Err(e) => {
              tracing::error!(?e, symbol = %k.symbol, "REST kline upsert failed");
              err += 1;
            }
          }
        }
        tracing::info!(upserted = ok, failed = err, "REST backfill persisted");
      }
      Err(e) => tracing::error!(?e, "REST fetch failed"),
    }
  });

  // Task B: WS subscribe btcusdt@kline_1m,closed kline 自動 upsert(long-running)
  let ws_pool = pool.clone();
  tokio::spawn(async move {
    if let Err(e) =
      binance::ws::subscribe_kline_stream(ws_pool, &ws_base, "BTCUSDT", "1m").await
    {
      tracing::error!(?e, "WS stream failed");
    }
  });

  // ─── HTTP server(Phase A-0 + A-2 /api/v1/klines)───
  let app = Router::new()
    .route("/healthz", get(healthz))
    .route("/api/v1/klines", get(api::klines::get_klines))
    .layer(TraceLayer::new_for_http())
    .with_state(pool);

  let addr: SocketAddr = "0.0.0.0:3000".parse()?;
  let listener = TcpListener::bind(addr).await?;
  tracing::info!(%addr, "listening");

  axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal())
    .await?;

  Ok(())
}

async fn healthz() -> Json<serde_json::Value> {
  Json(json!({ "status": "ok" }))
}

async fn shutdown_signal() {
  signal::ctrl_c()
    .await
    .expect("failed to install Ctrl+C handler");
  tracing::info!("shutdown signal received");
}
