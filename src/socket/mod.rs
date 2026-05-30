use serde::Deserialize;
use serde_json::json;
use socketioxide::{
  SocketIo,
  extract::{Data, SocketRef},
};
use tracing::info;

/// `subscribe` / `unsubscribe` payload(§6.5)。fire-and-forget(無 ack)。
#[derive(Debug, Deserialize)]
struct SubscriptionPayload {
  symbol: String,
  interval: String,
}

/// 註冊所有 socket.io namespace(§6.5:一條 TCP,兩個 namespace)。
///
/// - `/quote` — 高頻行情。client emit `subscribe { symbol, interval }` 加入 room
///   `${SYMBOL_UPPER}:${interval}`;`unsubscribe` 同 shape 離開;server 推
///   `kline` event 到對應 room(`binance::ws::subscribe_kline_stream` 來源)
/// - `/`      — 低頻全站訊號,client 接 `callUpdate`(事件風暴源頭,fan-out 推所有 client)
pub fn register_namespaces(io: &SocketIo) {
  io.ns("/quote", |s: SocketRef| async move {
    info!(id = %s.id, "client connected to /quote");

    // subscribe → join room `${SYMBOL_UPPER}:${interval}`(§6.5,無 ack;join 為 infallible)
    s.on(
      "subscribe",
      |s: SocketRef, Data::<SubscriptionPayload>(p)| async move {
        let room = format!("{}:{}", p.symbol.to_uppercase(), p.interval);
        s.join(room.clone());
        info!(id = %s.id, %room, "client subscribed to /quote room");
      },
    );

    // unsubscribe → leave room
    s.on(
      "unsubscribe",
      |s: SocketRef, Data::<SubscriptionPayload>(p)| async move {
        let room = format!("{}:{}", p.symbol.to_uppercase(), p.interval);
        s.leave(room.clone());
        info!(id = %s.id, %room, "client unsubscribed from /quote room");
      },
    );

    s.on_disconnect(|s: SocketRef| async move {
      info!(id = %s.id, "client disconnected from /quote");
    });
  });

  io.ns("/", |s: SocketRef| async move {
    info!(id = %s.id, "client connected to /");
    s.on_disconnect(|s: SocketRef| async move {
      info!(id = %s.id, "client disconnected from /");
    });
  });
}

/// emit 失敗的中性 marker。
///
/// 刻意不綁任何 HTTP error code:呼叫端自行 map —— manual `/admin/broadcast`
/// → `broadcast_failed`;CRUD mutation commit 後 → `committed_broadcast_failed`。
/// 這樣同一個 emit helper 不會把兩種語意混掉(§6.6 三態)。
pub struct BroadcastError;

/// 廣播 `callUpdate` 到 `/` namespace 所有 client(§6.5 fan-out:無過濾)。
///
/// `/` namespace 不存在或 emit 失敗都回 `Err(BroadcastError)`(原始錯誤只進 log)。
/// **不得**在此吞錯回成功 —— 由呼叫端決定對應的 error code。
pub async fn emit_call_update(io: &SocketIo) -> Result<(), BroadcastError> {
  let ns = io.of("/").ok_or_else(|| {
    tracing::warn!("emit callUpdate: / namespace not registered");
    BroadcastError
  })?;

  ns.emit(
    "callUpdate",
    &json!({
      "resource": "trading-pairs",
      "timestamp": chrono::Utc::now().timestamp_millis(),
    }),
  )
  .await
  .map_err(|e| {
    tracing::warn!(?e, "emit callUpdate failed");
    BroadcastError
  })
}
