use std::net::SocketAddr;

use anyhow::Result;
use axum::{
  Json, Router,
  extract::FromRef,
  middleware,
  response::Html,
  routing::{get, patch, post},
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
/// - `/admin` middleware 與 mutation handler 用 `State<AppState>` 取 `io` + `admin_token`
///   + `http`(共用 reqwest client,帶 timeout)+ `binance_rest_base`(POST 問幣安)
#[derive(Clone)]
pub struct AppState {
  pub pool: PgPool,
  pub io: SocketIo,
  pub admin_token: String,
  pub http: reqwest::Client,
  pub binance_rest_base: String,
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

/// 解析 `BINANCE_KLINE_SYMBOLS` 環境變數:逗號分隔,trim + uppercase,空字串略過,
/// 首次出現順序保留 dedupe;最終 list 為空 → `anyhow::bail!`(app refuses to start)。
///
/// 範例:`" btcusdt , shibusdt ,, btcusdt "` → `["BTCUSDT", "SHIBUSDT"]`。
fn parse_binance_kline_symbols(raw: &str) -> Result<Vec<String>> {
  let mut seen = std::collections::HashSet::new();
  let mut symbols = Vec::new();
  for part in raw.split(',') {
    let s = part.trim().to_uppercase();
    if s.is_empty() {
      continue;
    }
    if seen.insert(s.clone()) {
      symbols.push(s);
    }
  }
  if symbols.is_empty() {
    anyhow::bail!(
      "BINANCE_KLINE_SYMBOLS resolved to empty allowlist after trim/dedupe; refusing to start"
    );
  }
  Ok(symbols)
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

  // Binance K 線 ingestion allowlist:env 未設則用 code default(demo 開箱即用)。
  // 空 list(全 trim 後為空)→ parse fn 直接 anyhow::bail!,app 拒絕啟動。
  // P2:本階段 interval 固定 `1m`(symbol-only allowlist)。
  let kline_symbols_raw = std::env::var("BINANCE_KLINE_SYMBOLS")
    .unwrap_or_else(|_| "BTCUSDT,ETHUSDT,BNBUSDT,SOLUSDT,DOGEUSDT,SHIBUSDT".to_string());
  let kline_symbols = parse_binance_kline_symbols(&kline_symbols_raw)?;
  const KLINE_INTERVAL: &str = "1m";
  tracing::info!(
    symbols = ?kline_symbols,
    interval = KLINE_INTERVAL,
    count = kline_symbols.len(),
    "binance kline ingest allowlist resolved"
  );

  // 共用 HTTP client:設 timeout,否則「upstream timeout → 502」只是紙上規格(guardrail #1)
  let http = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(10))
    .build()?;

  // ─── Socket.IO layer ───
  let (socketio_layer, io) = SocketIo::new_layer();
  socket::register_namespaces(&io);

  // ─── Binance ingestion tasks(per-symbol)───
  // 每個 symbol 各自:Task A 一次性 REST backfill(500 根)+ Task B 常駐 WS reconnect loop。
  // P6:per-symbol 獨立 tokio::spawn —— 單 symbol 失敗只 log,不影響其他 symbol。
  for symbol in &kline_symbols {
    // Task A: REST backfill
    let rest_pool = pool.clone();
    let rest_http = http.clone();
    let rest_base_clone = rest_base.clone();
    let symbol_for_rest = symbol.clone();
    tokio::spawn(async move {
      match binance::rest::fetch_klines(
        &rest_http,
        &rest_base_clone,
        &symbol_for_rest,
        KLINE_INTERVAL,
        500,
      )
      .await
      {
        Ok(klines) => {
          tracing::info!(
            symbol = %symbol_for_rest,
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
          tracing::info!(symbol = %symbol_for_rest, upserted = ok, failed = err, "REST backfill persisted");
        }
        Err(e) => tracing::error!(?e, symbol = %symbol_for_rest, "REST fetch failed"),
      }
    });

    // Task B: WS reconnect loop(per-symbol,內含 5s→60s monotonic backoff + stable-reset,
    //         單 symbol failure log + 繼續;見 `binance::ws::run_with_reconnect` doc)。
    let ws_pool = pool.clone();
    let ws_io = io.clone();
    let ws_base_clone = ws_base.clone();
    let symbol_for_ws = symbol.clone();
    tokio::spawn(async move {
      binance::ws::run_with_reconnect(
        ws_pool,
        ws_io,
        ws_base_clone,
        symbol_for_ws,
        KLINE_INTERVAL.to_string(),
      )
      .await;
    });
  }

  // ─── HTTP server ───
  let app_state = AppState {
    pool,
    io,
    admin_token,
    http,
    binance_rest_base: rest_base,
  };

  // /admin/* 共用 token middleware(§8 C);trading-pairs CRUD route 加進這個 sub-router 自動沿用
  let admin_routes = Router::new()
    .route("/broadcast", post(api::admin::broadcast))
    .route(
      "/trading-pairs",
      get(api::trading_pairs::list_admin).post(api::trading_pairs::create),
    )
    .route(
      "/trading-pairs/{id}",
      patch(api::trading_pairs::update).delete(api::trading_pairs::delete),
    )
    .route("/audit/recent", get(api::trading_pairs::list_audit_recent))
    .route_layer(middleware::from_fn_with_state(
      app_state.clone(),
      api::admin::require_admin_token,
    ));

  let app = Router::new()
    .route("/healthz", get(healthz))
    .route("/api/v1/klines", get(api::klines::get_klines))
    .route(
      "/api/v1/trading-pairs",
      get(api::trading_pairs::list_public),
    )
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
