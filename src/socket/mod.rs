use socketioxide::{SocketIo, extract::SocketRef};
use tracing::info;

/// 註冊所有 socket.io namespace。
///
/// Phase A-3.1:
///   - `/quote` — 對外廣播實時行情(closed kline)。Client subscribe 後接 `kline:closed`。
///
/// Phase A-3.2+(尚未實作):
///   - `/` — public room,接 `callUpdate`(state 變更通知,§6.5 socket 協議)
pub fn register_namespaces(io: &SocketIo) {
  io.ns("/quote", |s: SocketRef| async move {
    info!(id = %s.id, "client connected to /quote");
    s.on_disconnect(|s: SocketRef| async move {
      info!(id = %s.id, "client disconnected from /quote");
    });
  });
}
