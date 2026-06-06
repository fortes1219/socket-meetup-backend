# Socket Meetup Backend

Rust backend for the Vue Meetup demo: **Socket realtime governance, K-line history/realtime lifecycle, and committed CRUD invalidation**.

This repository is not only a mock server for a chart demo. It provides the backend side of a frontend realtime-governance architecture:

- high-frequency K-line data through a dedicated `/quote` Socket.IO namespace
- low-frequency invalidation signals through the root `/` Socket.IO namespace
- REST K-line history backed by PostgreSQL
- Admin CRUD with strict validation, transaction, audit, commit, and post-commit `callUpdate`
- Binance ingestion where unclosed ticks are emitted as realtime presentation data and closed K-lines are persisted as history

The matching frontend repository uses this backend to demonstrate why `BroadcastChannel` should act as a **coordination/control plane**, not as a high-frequency market-data bus.

---

## Why this backend exists

The meetup demo is about this problem:

> When a product has multiple browser tabs, high-frequency realtime data, trading operations, and Admin CRUD updates, how do we decide who owns realtime, who refetches, and which data source is trusted?

The backend provides the event chain required to make that problem real:

```text
Binance WS / REST
→ Backend /quote high-frequency K-line data
→ Frontend realtime owner tab

Admin CRUD
→ DB transaction + audit
→ commit
→ Backend / low-frequency callUpdate
→ Frontend control coordinator refresh / snapshot / BroadcastChannel sync
```

---

## Architecture Overview

```text
src/
├── api/
│   ├── klines.rs              # REST history API
│   ├── trading_pairs.rs       # Public + Admin trading pair APIs
│   └── extractors.rs          # Strict request extraction / rejection mapping
├── binance/
│   ├── rest.rs                # REST backfill / exchangeInfo
│   └── ws.rs                  # Binance kline websocket ingestion
├── db/
│   ├── klines.rs              # BigDecimal DB boundary + kline upsert/read
│   └── trading_pairs.rs       # Trading pair queries + audit writes
├── socket/
│   └── mod.rs                 # Socket.IO namespaces and callUpdate emission
├── error.rs                   # AppError and API error envelope
└── main.rs                    # AppState, router, Socket.IO layer, startup tasks
```

---

## Socket Namespace Boundary

The backend intentionally separates high-frequency data from low-frequency control signals.

```text
/quote = high-frequency K-line data plane
/      = low-frequency control signal plane
```

### `/quote` namespace

Used for K-line realtime data.

Client emits:

```ts
subscribe({ symbol, interval })
unsubscribe({ symbol, interval })
```

Server behavior:

- joins the client into a room like `BTCUSDT:1m`
- emits `kline` events to that room
- does not broadcast Admin invalidation events here

This namespace represents the high-frequency data plane.

### `/` namespace

Used for low-frequency invalidation signals.

Server emits:

```ts
callUpdate({
  resource: 'trading-pairs',
  timestamp: number
})
```

This event is not the updated data body. It is a committed invalidation signal.

The frontend receives this signal and lets its control coordinator decide who should refresh and publish a snapshot.

---

## K-line Data Lifecycle

K-line data has two different meanings:

```text
unclosed tick = presentation realtime
closed kline  = persisted history
```

Backend ingestion flow:

1. REST backfill loads recent K-lines and upserts them into PostgreSQL.
2. Binance WebSocket streams realtime kline ticks.
3. Every tick can be emitted to `/quote` for realtime UI updates.
4. Only closed K-lines are upserted into DB.
5. Frontend resume reloads REST history, then reconnects realtime.

Why this matters:

- Unclosed ticks update the current candle.
- Closed K-lines become historical facts.
- Resume should use stable history, then reconnect realtime.
- Replayed closed WS events are safe because DB writes use upsert.

---

## Admin Mutation Lifecycle

Admin CRUD does not push full data into every frontend tab.

Flow:

```text
POST / PATCH / DELETE
→ strict validation
→ DB transaction
→ write trading_pairs
→ write audit
→ commit
→ emit callUpdate(resource)
→ frontend coordinator refreshes and publishes snapshot
```

Supported Admin APIs:

```text
POST   /admin/trading-pairs
PATCH  /admin/trading-pairs/:id
DELETE /admin/trading-pairs/:id
```

Important behavior:

- `POST` normalizes symbol and verifies base/quote through Binance `exchangeInfo`.
- `PATCH` rejects empty patches.
- `PATCH` only writes real changes.
- No-op patch does not update `updated_at` and does not emit `callUpdate`.
- Multiple field changes produce multiple audit rows.
- `DELETE` is soft delete plus audit.
- `callUpdate` is emitted only after DB commit.

---

## Committed Invalidation Signal

`callUpdate` means:

> The backend has committed a resource change. Frontend should invalidate or refresh its server state.

It does not mean:

- optimistic UI event
- full replacement data
- high-frequency data stream
- reason for all tabs to refetch immediately

If DB commit succeeds but broadcasting `callUpdate` fails, the mutation must not be retried blindly.

The backend distinguishes:

```text
broadcast_failed             # broadcast failed before a committed mutation context
committed_broadcast_failed   # mutation committed, but notification failed
```

This distinction matters because a committed mutation already changed the database.

The correct recovery path is refetch / sync, not retrying the write.

---

## Precision Boundary

Financial and market data should not casually become JavaScript floating numbers.

Backend rules:

- Binance wire values arrive as strings.
- DB stores numeric values as `BigDecimal`.
- REST API returns money/price/volume fields as strings.
- Fixed scale formatting avoids accidental display drift like `76907.00000000` becoming `76907`.

This keeps the boundary clear:

```text
wire string → BigDecimal in DB → API string → frontend parser/adapter
```

---

## Request Validation and Error Model

The backend avoids letting raw framework rejection messages leak to clients.

Request extraction is normalized through strict extractors:

- query/path/body rejection becomes `invalid_param`
- JSON unknown fields are rejected
- domain errors do not collapse into generic 500

Representative error codes:

```text
unauthorized
invalid_param
empty_patch
not_found
conflict
symbol_not_found
upstream_error
broadcast_failed
committed_broadcast_failed
internal_error
```

Raw SQL / upstream error chains are logged, not serialized into API responses.

---

## Database and Audit

The backend uses PostgreSQL through `sqlx`.

Migrations include:

```text
0001_init_trading_pairs.sql
0002_init_klines.sql
0003_trading_pair_constraints.sql
```

Trading pair design notes:

- Public list exposes only the fields required by the public UI.
- Admin list exposes operational fields.
- Soft-deleted trading pairs can still be referenced by audit history.
- Partial index for visible public list is expected to keep public reads cheap.

Audit records are written inside the same transaction as Admin mutation.

---

## Environment Variables

Expected variables may include:

```env
DATABASE_URL=postgres://postgres:postgres@localhost:5432/socket_meetup
ADMIN_TOKEN=change-me
BINANCE_REST_BASE=https://api.binance.com
BINANCE_WS_BASE=wss://stream.binance.com:9443
BINANCE_KLINE_SYMBOLS=BTCUSDT,ETHUSDT,ADAUSDT
HOST=127.0.0.1
PORT=3000
```

`BINANCE_KLINE_SYMBOLS` is normalized on startup:

- trim
- uppercase
- dedupe
- reject empty allowlist

---

## Local Development

Typical local flow:

```bash
cargo run
```

Or with Docker Compose if the project includes local Postgres and service wiring:

```bash
docker compose up --build
```

Run migrations before serving APIs:

```bash
sqlx migrate run
```

---

## API / Socket Summary

### Public APIs

```text
GET /api/v1/klines?symbol=BTCUSDT&interval=1m&limit=500
GET /api/v1/trading-pairs
```

### Admin APIs

```text
POST   /admin/trading-pairs
PATCH  /admin/trading-pairs/:id
DELETE /admin/trading-pairs/:id
GET    /admin/trading-pairs
GET    /admin/trading-pairs/audits/recent
```

### Socket.IO

```text
/quote
  subscribe
  unsubscribe
  kline

/
  callUpdate
```

---

## Frontend Integration Contract

Frontend should treat the backend events as two separate planes:

```text
/quote kline event
→ consumed only by the current realtime owner tab
→ updates chart presentation state

/ callUpdate event
→ consumed by status/control layer
→ triggers coordinator refresh/snapshot path
→ may sync followers through BroadcastChannel control messages
```

The frontend should not treat `callUpdate` as data.

The frontend should not fan out K-line ticks through BroadcastChannel by default.

---

## Why this matters

This backend supports a larger frontend architecture lesson:

```text
Not every tab should own realtime.
Not every socket event should become a refetch storm.
Not every update should be broadcast as data.
Not every AI-generated implementation should be trusted without contract and evidence.
```

In a trading-like product, frontend and backend must agree on:

- where realtime comes from
- what is considered history
- when mutation becomes committed fact
- how invalidation propagates
- how precision is serialized
- how failure states are distinguished

This is the backend half of that protocol.
