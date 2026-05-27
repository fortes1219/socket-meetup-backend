use socketioxide::{SocketIo, extract::SocketRef};
use tracing::info;

/// 註冊所有 socket.io namespace(§6.5:一條 TCP,兩個 namespace)。
///
/// - `/quote` — 高頻行情(closed kline),client subscribe 後接 `kline:closed`
/// - `/`      — 低頻全站訊號,client 接 `callUpdate`(事件風暴源頭,fan-out 推所有 client)
pub fn register_namespaces(io: &SocketIo) {
  io.ns("/quote", |s: SocketRef| async move {
    info!(id = %s.id, "client connected to /quote");
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
