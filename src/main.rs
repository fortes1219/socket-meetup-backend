use std::net::SocketAddr;

use anyhow::Result;
use axum::{Json, Router, routing::get};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod binance;

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
  let _pool = PgPoolOptions::new()
    .max_connections(5)
    .connect(&database_url)
    .await?;
  tracing::info!("connected to postgres");

  // ─── Phase A-1: Binance REST + WS demo(log 印,不落庫)───
  let rest_base = std::env::var("BINANCE_REST_BASE")
    .map_err(|_| anyhow::anyhow!("BINANCE_REST_BASE not set; check .env"))?;
  let ws_base = std::env::var("BINANCE_WS_BASE")
    .map_err(|_| anyhow::anyhow!("BINANCE_WS_BASE not set; check .env"))?;

  // Task A: REST 撈 5 筆,sanity check(跑完 task 結束)
  tokio::spawn(async move {
    let client = reqwest::Client::new();
    match binance::rest::fetch_klines(&client, &rest_base, "BTCUSDT", "1m", 5).await {
      Ok(klines) => tracing::info!(
        count = klines.len(),
        last_close = %klines.last().map(|k| k.close.as_str()).unwrap_or("(empty)"),
        "REST fetched"
      ),
      Err(e) => tracing::error!(?e, "REST fetch failed"),
    }
  });

  // Task B: WS subscribe btcusdt@kline_1m(long-running)
  tokio::spawn(async move {
    if let Err(e) = binance::ws::subscribe_kline_stream(&ws_base, "BTCUSDT", "1m").await {
      tracing::error!(?e, "WS stream failed");
    }
  });

  // ─── HTTP server(Phase A-0 既有)───
  let app = Router::new()
    .route("/healthz", get(healthz))
    .layer(TraceLayer::new_for_http());

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
