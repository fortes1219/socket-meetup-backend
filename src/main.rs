use std::net::SocketAddr;

use anyhow::Result;
use axum::{
  Json, Router,
  extract::FromRef,
  middleware,
  response::Html,
  routing::{get, post},
};
use serde_json::json;
use socketioxide::SocketIo;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod api;
mod binance;
mod db;
mod error;
mod socket;

/// Composite state。
///
/// - 既有 handler 透過 `FromRef` 取 `State<PgPool>` / `State<SocketIo>`
/// - `/admin` middleware 與 handler 用 `State<AppState>` 取 `io` + `admin_token`
#[derive(Clone)]
pub struct AppState {
  pub pool: PgPool,
  pub io: SocketIo,
  pub admin_token: String,
}

impl FromRef<AppState> for PgPool {
  fn from_ref(state: &AppState) -> Self {
    state.pool.clone()
  }
}

impl FromRef<AppState> for SocketIo {
  fn from_ref(state: &AppState) -> Self {
    state.io.clone()
  }
}

#[tokio::main]
async fn main() -> Result<()> {
  // Load .env(本機 dev;production env 由外部注入,沒 .env 不 panic)
  dotenvy::dotenv().ok();

  // Tracing(讀 RUST_LOG,fallback info + sqlx warn + socketio/engineio warn 抑制 noise)
  tracing_subscriber::fmt()
    .with_env_filter(
      EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,socketioxide=warn,engineioxide=warn")),
    )
    .init();

  // Postgres pool
  let database_url = std::env::var("DATABASE_URL")
    .map_err(|_| anyhow::anyhow!("DATABASE_URL not set; check .env"))?;
  let pool = PgPoolOptions::new()
    .max_connections(5)
    .connect(&database_url)
    .await?;
  tracing::info!("connected to postgres");

  // Binance bases + admin token
  let rest_base = std::env::var("BINANCE_REST_BASE")
    .map_err(|_| anyhow::anyhow!("BINANCE_REST_BASE not set; check .env"))?;
  let ws_base = std::env::var("BINANCE_WS_BASE")
    .map_err(|_| anyhow::anyhow!("BINANCE_WS_BASE not set; check .env"))?;
  let admin_token =
    std::env::var("ADMIN_TOKEN").map_err(|_| anyhow::anyhow!("ADMIN_TOKEN not set; check .env"))?;

  // ─── Socket.IO layer ───
  let (socketio_layer, io) = SocketIo::new_layer();
  socket::register_namespaces(&io);

  // ─── Binance ingestion tasks ───
  // Task A: REST 撈 500 筆 backfill(一次性)
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

  // Task B: WS subscribe btcusdt@kline_1m,closed kline upsert + emit `kline:closed`
  let ws_pool = pool.clone();
  let ws_io = io.clone();
  tokio::spawn(async move {
    if let Err(e) =
      binance::ws::subscribe_kline_stream(ws_pool, ws_io, &ws_base, "BTCUSDT", "1m").await
    {
      tracing::error!(?e, "WS stream failed");
    }
  });

  // ─── HTTP server ───
  let app_state = AppState {
    pool,
    io,
    admin_token,
  };

  // /admin/* 共用 token middleware(§8 C);A-3.3 CRUD route 加進這個 sub-router 自動沿用
  let admin_routes = Router::new()
    .route("/broadcast", post(api::admin::broadcast))
    .route_layer(middleware::from_fn_with_state(
      app_state.clone(),
      api::admin::require_admin_token,
    ));

  let app = Router::new()
    .route("/healthz", get(healthz))
    .route("/api/v1/klines", get(api::klines::get_klines))
    .route("/socket-test", get(socket_test_page))
    .nest("/admin", admin_routes)
    .layer(socketio_layer)
    .layer(TraceLayer::new_for_http())
    .with_state(app_state);

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

/// Browser test page,demo 用:http://localhost:3000/socket-test
async fn socket_test_page() -> Html<&'static str> {
  Html(include_str!("../test/socket-client.html"))
}

async fn shutdown_signal() {
  signal::ctrl_c()
    .await
    .expect("failed to install Ctrl+C handler");
  tracing::info!("shutdown signal received");
}
