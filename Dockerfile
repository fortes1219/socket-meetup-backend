# ─── Stage 1: builder(編譯 release binary)───
FROM rust:1.95-bookworm AS builder

# sqlx::query! 編譯期走 offline cache(.sqlx/),不連 DB(Docker build 隔離無 postgres)
ENV SQLX_OFFLINE=true

WORKDIR /app

# Layer cache trick:先 fetch deps,只有 Cargo.toml/Cargo.lock 變才重 fetch
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release --locked && \
    rm -rf src target/release/socket-meetup-backend* \
           target/release/deps/socket_meetup_backend*

# 真正的 source + sqlx offline cache(.sqlx)+ test page(main.rs include_str!)
COPY .sqlx ./.sqlx
COPY src ./src
COPY test ./test
RUN cargo build --release --locked

# ─── Stage 2: runtime(只放 binary + CA bundle)───
FROM debian:12-slim AS runtime

# rustls 要 system CA bundle 才能驗證 binance.com 的 HTTPS
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/socket-meetup-backend ./socket-meetup-backend

EXPOSE 3000
ENTRYPOINT ["./socket-meetup-backend"]
