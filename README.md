# socket-meetup-backend

Vue Meetup 2026-06-14 demo 後端。Rust + axum + socketioxide,示範 socket 事件風暴下的連線收斂(Leader 選舉)與事件節流。

## Stack

- axum 0.8 + socketioxide 0.18.2
- PostgreSQL 16 + sqlx 0.8
- tokio-tungstenite 0.28(Binance 行情)

## 本地開發

```bash
cp .env.example .env
docker compose up -d
cargo run
```

## 對應前端

`socket-meetup-frontend`(Phase B 開立)
