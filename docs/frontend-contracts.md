# Frontend Contracts — `socket-meetup-backend`

> 給 `socket-meetup-frontend` repo Phase B/C/D 使用的 contract handoff。
> 前端據此建 Zod schema、ts-rest contract、TanStack Query key、socket adapter、klinecharts v10 整合。

---

## 1. 文件目的

- **不是**後端架構文件(那份是 `ARCHITECTURE.md`,合約源頭)
- **是** Phase B/C/D 前端串接的 contract 整理稿,把後端 wire surface 拆成前端可直接落地的 schema、key、adapter
- 任何後端 contract 變動 → 先回 `ARCHITECTURE.md` §6.5 / §6.6 / §6.7,再更新本檔
- 本檔不放後端實作細節(transaction、emit timing、SQL 等),只放前端會「打 / 收 / 解析」到的 shape

---

## 2. 全域約定(先記住,後面不再重複)

| 項目 | 規則 |
|---|---|
| auth header | admin 路徑統一 `X-Admin-Token: <value>`(`/admin/*` 全段);其他無 |
| timestamp | 一律 **integer ms epoch**(REST 與 socket 都是) |
| 金額 / OHLCV | wire 統一 **decimal string**(`"73512.00000000"`),避免 JS Number 精度損失;klinecharts adapter 才轉 `Number` |
| 命名 case 不一致 | **REST 全 snake_case**(`open_time`、`base_asset`、`display_order`);**`/quote` socket 用 camelCase**(`openTime`);**`/` socket callUpdate 用 plain key**(`resource`/`timestamp`)。**前端要在 Zod parse 邊界吸收差異** |
| ErrorBody 統一 shape | `{ error: <enum>, message: <string> }`(JSON);唯一例外見「§3.2 `/api/v1/klines`」query parse 失敗那段 |
| `deny_unknown_fields` 範圍 | `/admin/trading-pairs` 系列、`/admin/audit/recent`(query 與 body 都嚴格);`/api/v1/klines` query **未** strict(向後兼容)。 |
| timestamp 來源 | 後端 `chrono::Utc::now()`(server clock);前端不要假設與 client clock 對齊 |

### ErrorBody enum codes(完整集合)

| code | 意義 | typical status |
|---|---|---|
| `unauthorized` | 缺 / 錯 X-Admin-Token | 401 |
| `invalid_param` | strict rejection(query/path/body parse fail、值超範圍、unknown field、bad bool/uuid) | 400 |
| `empty_patch` | PATCH body 全 None(欄位都沒給) | 400 |
| `not_found` | `:id` 不存在或已 soft-deleted | 404 |
| `conflict` | symbol UNIQUE 違反(含 soft-deleted 佔用) | 409 |
| `symbol_not_found` | 幣安 exchangeInfo 認不得該 symbol(空 symbols 或 code -1121) | 422 |
| `upstream_error` | 幣安 network / timeout / non-2xx / 解析失敗 | 502 |
| `broadcast_failed` | 手動 `/admin/broadcast` emit 失敗或 `/` ns 不存在(**無** DB mutation) | 500 |
| `committed_broadcast_failed` | mutation **已 commit**,emit failed → 前端 refetch,**不得 retry mutation** | 500 |
| `internal_error` | 未預期內部錯誤(commit 前 DB failure 等) | 500 |

`message` 為固定可控人類可讀文字,**不要拿來當 i18n / branch key**;前端的分流邏輯只看 `error` 欄。

---

## 3. REST API contracts

### 3.1 `GET /healthz`

| 項 | 值 |
|---|---|
| 用途 | liveness probe,demo / k8s 用 |
| auth | 無 |
| request | 無 |
| success 200 | `{ "status": "ok" }` |
| error | 通常無;deploy 中可能拒連 |
| side effects | 無 |
| TanStack Query | 不建議走 Query;當 ping 用即可 |

### 3.2 `GET /api/v1/klines`

| 項 | 值 |
|---|---|
| 用途 | 取 K 線歷史,for klinecharts `setDataLoader.getBars`(init / forward) |
| auth | 無 |
| request query | `symbol: string`(required)/ `interval: string`(required,binance 風格 `"1m"` 等)/ `limit?: integer = 500` / **`endTime?: integer ms epoch`**(camelCase!) |
| success 200 | `Kline[]`(snake_case,**金額 string**,timestamp ms):`open_time` / `close_time` / `open` / `high` / `low` / `close` / `volume` / `trades_count`。**排序 `open_time DESC`**(最新在前);klinecharts 要 ascending,**adapter 必須 sort**(見 §6.4) |
| error | **400(plain-text)** 若 query parse 失敗(本 endpoint 未走 StrictQuery,行為與 `/admin/*` 不一致);**500 JSON `internal_error`** 若 DB failure |
| side effects | 無 |
| TanStack Query | key 建議:`["klines", { symbol, interval, limit, endTime }]`(`endTime` 為 `init` 撈最新時要省略);不設定 `staleTime: 0`,getBars 由 klinecharts 觸發即可 |
| 邊界 | `limit` 預設 500;`endTime` 省略 = init(最新 `limit` 筆);提供 = forward(該 ms 之前 `limit` 筆);**不支援 backward**(§6.7 規則,不在 frontend 動) |
| 覆蓋 symbol | 本 handler **不檢查 symbol 是否在 allowlist** —— 純 DB query。backend `BINANCE_KLINE_SYMBOLS`(預設 BTCUSDT / ETHUSDT / BNBUSDT / SOLUSDT / DOGEUSDT / SHIBUSDT)**只控制 startup REST backfill + 持續 WS stream 覆蓋範圍**;DB 內可能還有舊 symbol 的歷史資料(例如過去在 allowlist),REST 仍會照回。allowlist 外且 DB 也沒資料 → 自然 `200 []`。**前端不可把「在 allowlist 外」當「保證 []」**。詳見 §8 #1 |

### 3.3 `GET /api/v1/trading-pairs`

| 項 | 值 |
|---|---|
| 用途 | 公開:可選 trading pair 清單(只 `enabled && !deleted`) |
| auth | 無 |
| request | 無 |
| success 200 | `PublicTradingPair[]`,固定 4 欄,排序 `display_order ASC, symbol ASC` |
| error | 500 JSON `internal_error` |
| side effects | 無 |
| TanStack Query | key:`["trading-pairs", "public"]`;**callUpdate(`resource:"trading-pairs"`)收到即 invalidate** |

### 3.4 `GET /admin/trading-pairs?include_disabled=`

| 項 | 值 |
|---|---|
| 用途 | 後台:含 disabled 的 pair 管理清單 |
| auth | **X-Admin-Token** |
| request query | `include_disabled?: boolean = true`(deny_unknown_fields;非 bool / 未知欄 → 400 `invalid_param`)。**預設 true**(POST 新增的 pair 預設 `enabled=false`,後台預設要看得到) |
| success 200 | `AdminTradingPair[]`,排序 `display_order ASC, symbol ASC` |
| error | 400 `invalid_param` / 401 `unauthorized` / 500 `internal_error` |
| side effects | 無 |
| TanStack Query | key:`["trading-pairs", "admin", { include_disabled }]`;**callUpdate(`resource:"trading-pairs"`)收到即 invalidate** |

### 3.5 `POST /admin/trading-pairs`

| 項 | 值 |
|---|---|
| 用途 | 新增 trading pair。後端會 **tx 外問幣安 exchangeInfo** 取 `base_asset` / `quote_asset`,client 只給 `symbol` |
| auth | **X-Admin-Token** |
| request body | `{ symbol: string }`(deny_unknown_fields)。後端 **trim + uppercase**(`"btcusdt " → "BTCUSDT"`);trim 後空 → 400 `invalid_param` |
| success 201 | `AdminTradingPair`(新增的 pair,`enabled = false`、`display_order = 0`、`created_at == updated_at`) |
| error | 400 `invalid_param` / 401 / **409 `conflict`**(symbol 已存在,**含 soft-deleted** 佔用)/ **422 `symbol_not_found`**(幣安拒)/ **502 `upstream_error`**(幣安 network/timeout/5xx)/ 500 `internal_error` / 500 `committed_broadcast_failed` |
| side effects | INSERT pair + audit `added`(同 tx),**commit 後** emit `/` namespace `callUpdate { resource: "trading-pairs", timestamp }` |
| TanStack Query | mutation success → 直接 invalidate `["trading-pairs", *]`;callUpdate 進來會再 invalidate 一次(冪等);**`committed_broadcast_failed` = 已寫入但通知失敗,前端 refetch、不得 retry mutation**(見 §7) |

### 3.6 `PATCH /admin/trading-pairs/{id}`

| 項 | 值 |
|---|---|
| 用途 | 切換 enabled / 改 display_order |
| auth | **X-Admin-Token** |
| request path | `id: uuid`(malformed uuid → 400 `invalid_param`) |
| request body | `{ enabled?: boolean, display_order?: integer >= 0 }`(deny_unknown_fields)。**空 body `{}` → 400 `empty_patch`**(不同於 invalid_param!);`display_order < 0` → 400 `invalid_param` |
| success 200 | `AdminTradingPair`(更新後的 pair) |
| error | 400 `invalid_param` / 400 `empty_patch` / 401 / **404 `not_found`**(`:id` 不存在或已 soft-deleted)/ 500 `internal_error` / 500 `committed_broadcast_failed` |
| side effects | **no-op 不寫不發**(欄位有值但與現值相同 → 200,不 audit、不改 updated_at、不 emit);**實際變更**:UPDATE + 每個變更欄位 1 筆 audit(兩欄都變寫兩筆),**commit 後** emit `callUpdate` |
| TanStack Query | 同 POST;**前端可預先檢查 `{}` 並直接報錯**避免送一個 server 一定拒的 request,但不必要(server 端強檢) |

### 3.7 `DELETE /admin/trading-pairs/{id}`

| 項 | 值 |
|---|---|
| 用途 | soft delete trading pair |
| auth | **X-Admin-Token** |
| request path | `id: uuid` |
| request body | 無 |
| success 204 | 無 body |
| error | 401 / 404 `not_found` / 500 `internal_error` / 500 `committed_broadcast_failed` |
| side effects | `UPDATE deleted_at=now, updated_at=now` + audit `removed`(同 tx),commit 後 emit `callUpdate`。**absolutely soft**,不 hard DELETE;同 symbol re-POST 仍 **409 `conflict`**(UNIQUE 含 soft-deleted) |
| TanStack Query | 同 PATCH/POST |

### 3.8 `GET /admin/audit/recent?limit=`

| 項 | 值 |
|---|---|
| 用途 | 取最近 audit 紀錄(管理 UI / debug) |
| auth | **X-Admin-Token** |
| request query | `limit?: integer (1..=200) = 50`(deny_unknown_fields)。**`<= 0` 或 `> 200` → 400 `invalid_param`**(server **不 clamp**)。非 integer / 未知欄 → 400 `invalid_param` |
| success 200 | `AuditEntry[]`,排序 `occurred_at DESC, audit_id DESC`(tie-breaker 是 UUIDv7 時序單調)|
| error | 400 `invalid_param` / 401 / 500 `internal_error` |
| side effects | 無 |
| TanStack Query | key:`["trading-pairs", "audit-recent", { limit }]`;callUpdate 收到即 invalidate |
| 邊界 | **soft-deleted pair 的 `removed` audit 仍會回**(INNER JOIN 不過濾 `deleted_at`);`symbol` 從 join 取,即使 pair 被刪也看得到 |

### 3.9 `POST /admin/broadcast`

| 項 | 值 |
|---|---|
| 用途 | **demo 用**:手動觸發 `callUpdate`,不動資料,用來演示「事件風暴」 |
| auth | **X-Admin-Token** |
| request | 無 body |
| success 204 | 無 body |
| error | 401 / 500 `broadcast_failed`(emit fail 或 `/` ns 不存在) |
| side effects | emit `/` namespace `callUpdate { resource: "trading-pairs", timestamp }` 給所有連線 client(**無 room 過濾**) |
| TanStack Query | N/A;按下後 demo 預期所有 tab 同時 invalidate |

---

## 4. Zod schema 範例

```ts
import { z } from 'zod';

// ─── 全域 ─────────────────────────────────────────────────────────────

export const ErrorCodeEnum = z.enum([
  'unauthorized',
  'invalid_param',
  'empty_patch',
  'not_found',
  'conflict',
  'symbol_not_found',
  'upstream_error',
  'broadcast_failed',
  'committed_broadcast_failed',
  'internal_error',
]);
export type ErrorCode = z.infer<typeof ErrorCodeEnum>;

export const ErrorBodySchema = z.object({
  error: ErrorCodeEnum,
  message: z.string(),
}).strict();
export type ErrorBody = z.infer<typeof ErrorBodySchema>;

// ─── REST DTOs(snake_case,§6.6)──────────────────────────────────────
//
// 全部 .strict():後端契約沒帶的欄位 = 後端 contract drift,Zod 應該炸,
// 不可 silently strip(否則 drift 進來時前端發現不了)。

export const PublicTradingPairSchema = z.object({
  symbol: z.string(),
  base_asset: z.string(),
  quote_asset: z.string(),
  display_order: z.number().int(),
}).strict();

export const AdminTradingPairSchema = z.object({
  id: z.string().uuid(),
  symbol: z.string(),
  base_asset: z.string(),
  quote_asset: z.string(),
  enabled: z.boolean(),
  display_order: z.number().int().nonnegative(),
  created_at: z.number().int(), // ms epoch
  updated_at: z.number().int(),
}).strict();

export const AuditActionEnum = z.enum([
  'added', 'enabled', 'disabled', 'removed', 'reordered',
]);

export const AuditEntrySchema = z.object({
  audit_id: z.string().uuid(),
  trading_pair_id: z.string().uuid(),
  symbol: z.string(),
  action: AuditActionEnum,
  changed_by: z.string(),
  occurred_at: z.number().int(), // ms epoch
}).strict();

export const KlineSchema = z.object({
  open_time: z.number().int(),  // ms epoch
  close_time: z.number().int(),
  open: z.string(),  // decimal string,前端 Number() 轉
  high: z.string(),
  low: z.string(),
  close: z.string(),
  volume: z.string(),
  trades_count: z.number().int(),
}).strict();

// ─── REST request body(deny_unknown_fields → .strict())─────────────

// symbol:對齊後端的 trim + uppercase normalize,whitespace-only 在前端先擋
// (server 也會擋 400 invalid_param,但前端不要送一個自己一定要拒的 request)
export const CreateTradingPairBodySchema = z.object({
  symbol: z
    .string()
    .transform((s) => s.trim().toUpperCase())
    .refine((s) => s.length > 0, { message: 'symbol must not be empty after trim' }),
}).strict();

export const PatchTradingPairBodySchema = z.object({
  enabled: z.boolean().optional(),
  display_order: z.number().int().nonnegative().optional(),
}).strict();
// 注意:`{}` 本 schema parse 過,但 server 會回 400 empty_patch
// 前端可預先檢查(可選);也可由 server 做唯一仲裁

// ─── REST query schema ──────────────────────────────────────────────

export const AdminListQuerySchema = z.object({
  include_disabled: z.boolean().optional(),
}).strict();

export const AuditRecentQuerySchema = z.object({
  limit: z.number().int().min(1).max(200).optional(),
}).strict();

// ─── Socket payloads(全部 .strict(),contract drift 偵測)────────────

// `/` namespace
export const CallUpdatePayloadSchema = z.object({
  resource: z.literal('trading-pairs'),
  timestamp: z.number().int(),
}).strict();
export type CallUpdatePayload = z.infer<typeof CallUpdatePayloadSchema>;

// `/quote` namespace(**注意 camelCase**)
export const QuoteKlineEventSchema = z.object({
  symbol: z.string(),
  interval: z.string(),
  kline: z.object({
    openTime: z.number().int(),  // camelCase!
    open: z.string(),
    high: z.string(),
    low: z.string(),
    close: z.string(),
    volume: z.string(),
    closed: z.boolean(),
  }).strict(),
}).strict();
export type QuoteKlineEvent = z.infer<typeof QuoteKlineEventSchema>;
```

> **REST 與 socket 命名差異提醒:** `KlineSchema`(REST `/api/v1/klines`)是 **`open_time` snake_case**;`QuoteKlineEventSchema`(socket `/quote`)是 **`openTime` camelCase**。這是後端契約刻意如此(§6.5 / §6.6),前端兩個 schema 都建,**不要試圖共用**。

---

## 5. Socket.IO contracts

> 同一條 TCP、兩個 namespace(§6.5)。socket.io-client 4.x。Node runtime 建議 `transports: ["websocket"]`(避開 polling 邊角)。

### 5.1 `/` namespace —— 全站訊號

| 方向 | event | payload | 行為 |
|---|---|---|---|
| server → client | `callUpdate` | `{ resource: "trading-pairs", timestamp: integer ms }` | **fan-out 到所有連線 client,無 room 過濾**。觸發來源:`/admin/*` mutation commit 後、或手動 `POST /admin/broadcast` |

**前端動作:** 收到 → 視為「server-side 有 trading-pairs 相關變動」→ invalidate `["trading-pairs", *]`(public / admin / audit-recent)。**不需 ack**。

`resource` 目前只有 `"trading-pairs"`(demo scope);未來擴增前先回 §6.5。

### 5.2 `/quote` namespace —— K 線 realtime

| 方向 | event | body / payload | 行為 |
|---|---|---|---|
| client → server | `subscribe` | `{ symbol: string, interval: string }` | server `join(${SYMBOL_UPPER}:${interval})`,**fire-and-forget 無 ack**;server log 加入紀錄;**未驗 symbol/interval 合法性**,錯了只會 silently 不收 |
| client → server | `unsubscribe` | `{ symbol: string, interval: string }` | server `leave(room)`,同樣無 ack;斷線會自動 leave 所有 room |
| server → client | `kline` | 見下 | **room-filtered**:只推給 `${SYMBOL_UPPER}:${interval}` room 內 client。closed AND unclosed 都 emit,`closed` 旗標攜帶 |

`kline` payload(camelCase nested):

```ts
{
  symbol: "BTCUSDT",
  interval: "1m",
  kline: {
    openTime: 1780131960000,  // ms epoch
    open:   "73514.00000000",  // decimal string
    high:   "73514.01000000",
    low:    "73514.00000000",
    close:  "73514.01000000",
    volume: "0.21389000",
    closed: false              // true = bar 收盤;false = 進行中
  }
}
```

**room key 規則:**
- 統一格式 `"${SYMBOL_UPPER}:${interval}"`,例 `"BTCUSDT:1m"`
- frontend emit `subscribe` 前必須先 normalize(`symbol.trim().toUpperCase()`、`interval` 直給 binance 格式 `"1m"` 等)
- server 收到 payload 後也會 `symbol.to_uppercase()`,但**前端不要依賴 server normalize**(見 §7)

**`/quote` **絕對不**直接吐 klinecharts `KLineData`:** wire 是 `QuoteKlineEvent`(string OHLCV);klinecharts adapter 才轉(§6)。**socket 層不 import klinecharts。**

**已知行為:**
- 連上 `/quote` 但未 `subscribe` → 收不到任何 `kline` event(room 隔離)
- subscribe **`BINANCE_KLINE_SYMBOLS` allowlist 外**的 symbol(例 `ADAUSDT:1m`,非預設 6 個之一)→ 加入 room 成功,但**沒有 tick**(見 §8 #1)
- 斷線重連後,**前次 subscribe 不會自動恢復**;前端需要在 `connect` event 重新 emit

---

## 6. klinecharts v10 adapter

> 三層分離(§6.5 / §6.7):wire (`/quote`) ↔ adapter ↔ klinecharts。socket 層不耦合 klinecharts。

### 6.1 v10 type 參考(從 `klinecharts@10.0.0-beta1/dist/index.d.ts` 取得)

```ts
// 摘錄 — 完整見 klinecharts package
export type Timestamp = number;

export interface KLineData {
  timestamp: Timestamp;  // ms epoch
  open: number;
  high: number;
  low: number;
  close: number;
  volume?: number;
  turnover?: number;
}

export interface SymbolInfo {
  ticker: string;
  pricePrecision: number;
  volumePrecision: number;
}
export interface Period {
  type: 'second' | 'minute' | 'hour' | 'day' | 'week' | 'month' | 'year';
  span: number;
}

export interface DataLoader {
  getBars: (params: {
    type: 'init' | 'forward' | 'backward' | 'update';
    timestamp: number | null;
    symbol: SymbolInfo;
    period: Period;
    callback: (data: KLineData[], more?: { forward?: boolean; backward?: boolean }) => void;
  }) => void | Promise<void>;
  subscribeBar?: (params: {
    symbol: SymbolInfo;
    period: Period;
    callback: (data: KLineData) => void;  // 單筆!非 array
  }) => void;
  unsubscribeBar?: (params: { symbol: SymbolInfo; period: Period }) => void;
}
```

### 6.2 v10 subscribeBar Data Merge 規則(官方 docs)

`subscribeBar.callback(data)` 連續呼叫:

| `data.timestamp` 與最新 bar 比較 | 行為 |
|---|---|
| greater | **append** 新 bar |
| same | **overwrite** 最新 bar(realtime 更新進行中 bar) |
| smaller | **ignore**(防 stale tick) |

→ frontend 把每筆 wire tick 都直接餵給 callback,closed/unclosed 由 `timestamp` 決定 update vs append,**不需要前端自己 dedup**。`closed` flag 不用傳給 klinecharts(`KLineData` 沒這欄)。

### 6.3 adapter 範例(TypeScript)

```ts
import type { KLineData } from 'klinecharts';
import type { QuoteKlineEvent } from './contracts';

// wire 層 wire-shape(socket 層之上,不沾 klinecharts)
export interface KlineTick {
  symbol: string;
  interval: string;
  openTime: number;
  open: string; high: string; low: string; close: string; volume: string;
  closed: boolean;  // 給 store / debug,不傳 klinecharts
}

// `/quote` event → KlineTick(flatten)
export function eventToTick(e: QuoteKlineEvent): KlineTick {
  return {
    symbol: e.symbol,
    interval: e.interval,
    openTime: e.kline.openTime,
    open: e.kline.open,
    high: e.kline.high,
    low: e.kline.low,
    close: e.kline.close,
    volume: e.kline.volume,
    closed: e.kline.closed,
  };
}

// KlineTick → klinecharts KLineData
//  - timestamp: openTime(ms,已對)
//  - OHLCV: string → Number(精度由 setSymbol 的 pricePrecision/volumePrecision 控)
//  - closed: 不傳
export function tickToKLineData(t: KlineTick): KLineData {
  return {
    timestamp: t.openTime,
    open:   Number(t.open),
    high:   Number(t.high),
    low:    Number(t.low),
    close:  Number(t.close),
    volume: Number(t.volume),
  };
}

// klinecharts `Period` → binance interval string
export function periodToBinanceInterval(p: { type: string; span: number }): string {
  switch (p.type) {
    case 'minute': return `${p.span}m`;
    case 'hour':   return `${p.span}h`;
    case 'day':    return `${p.span}d`;
    case 'week':   return `${p.span}w`;
    case 'month':  return `${p.span}M`;
    default: throw new Error(`unsupported period type: ${p.type}`);
  }
}
```

### 6.4 setDataLoader 三函數實作骨架

```ts
import { tickToKLineData, periodToBinanceInterval } from './adapter';
import { KlineSchema } from './contracts';  // Zod schemas

let currentToken: symbol = Symbol();  // §6.7 stale-subscription guard

const dataLoader = {
  async getBars({ type, timestamp, symbol, period, callback }) {
    if (type === 'backward') {
      callback([], {});  // demo 不支援 backward
      return;
    }
    const interval = periodToBinanceInterval(period);
    const url = new URL('/api/v1/klines', window.location.origin);
    url.searchParams.set('symbol', symbol.ticker);
    url.searchParams.set('interval', interval);
    url.searchParams.set('limit', '500');
    if (type === 'forward' && timestamp != null) {
      url.searchParams.set('endTime', String(timestamp));  // camelCase!
    }
    const json = await fetch(url).then((r) => r.json());
    // contract-first:Zod parse 兼做 contract drift 檢查(strict schema 會炸未知欄)
    const klines = KlineSchema.array().parse(json);
    // 後端回 `open_time DESC`(最新在前);klinecharts callback 要 ascending → sort
    const bars: KLineData[] = klines
      .map((k) => ({
        timestamp: k.open_time,
        open:   Number(k.open),
        high:   Number(k.high),
        low:    Number(k.low),
        close:  Number(k.close),
        volume: Number(k.volume),
      }))
      .sort((a, b) => a.timestamp - b.timestamp);
    const hasMore = klines.length === 500;
    callback(bars, { forward: hasMore });
  },

  subscribeBar({ symbol, period, callback }) {
    const token = Symbol();
    currentToken = token;
    const interval = periodToBinanceInterval(period);
    quoteSocket.subscribe(symbol.ticker, interval, (tick) => {
      if (currentToken !== token) return;  // stale,丟掉
      callback(tickToKLineData(tick));
    });
  },

  unsubscribeBar({ symbol, period }) {
    currentToken = Symbol();  // invalidate 既有 callback
    const interval = periodToBinanceInterval(period);
    quoteSocket.unsubscribe(symbol.ticker, interval);
  },
};
```

`quoteSocket.subscribe` / `unsubscribe` 是 store 層的 wrapper(emit `subscribe`/`unsubscribe` + 註冊 callback by `${SYMBOL_UPPER}:${interval}`);**store 層不 import klinecharts**。

---

## 7. TanStack Query — key / invalidation 建議

### 7.1 Query keys

| Resource | key |
|---|---|
| 公開 trading pairs | `["trading-pairs", "public"]` |
| 後台 trading pairs | `["trading-pairs", "admin", { include_disabled }]` |
| Audit recent | `["trading-pairs", "audit-recent", { limit }]` |
| K-line history | `["klines", { symbol, interval, limit, endTime }]`(getBars 內走 fetch,不一定要進 TanStack) |

### 7.2 `/` namespace `callUpdate` 收到後

```ts
socket.on('callUpdate', (raw) => {
  const e = CallUpdatePayloadSchema.parse(raw);
  if (e.resource === 'trading-pairs') {
    queryClient.invalidateQueries({ queryKey: ['trading-pairs'] });
  }
});
```

無 throttle、無 dedup;事件來幾次 invalidate 幾次(TanStack 自會 dedupe 實際 refetch)。`/quote` 的 `kline` event **不**走 invalidation,直接餵 klinecharts。

### 7.3 CRUD mutation 成功後的 invalidation 策略

- **後端會 emit `callUpdate`**,所以 invalidation 一定會被觸發(經 socket)
- 但 demo 推薦 **mutation onSuccess 直接 invalidate**,**再** 由 callUpdate 補一次(TanStack 端冪等):
  - 好處:`onSuccess` 的 invalidate 立刻生效,不需要等 socket round-trip
  - callUpdate 再補一次 → 雙保險(若 socket lag、其他 tab 連線)
- **`committed_broadcast_failed`(500)發生時:** 後端已 commit,但 emit 失敗 → 前端要把它當「mutation 成功 + 廣播失敗」處理:
  - **必須 refetch**(invalidate `["trading-pairs"]`)
  - **不得 retry mutation**(會再 INSERT/UPDATE 一次造成髒資料)
  - UI 可顯示「已存,但其他頁面通知失敗」

### 7.4 §5.8 cross-tab coherence

ARCHITECTURE.md §5.8 定義跨 tab 的 BroadcastChannel + leader 接力 protocol(`control:invalidated` / `control:updated` / `sync-request`);本檔不重述,Phase C 開工時請翻 §5.8。

---

## 8. Known limitations / 風險

| # | 限制 | 影響 |
|---|---|---|
| 1 | **`BINANCE_KLINE_SYMBOLS` 決定後端 ingestion 範圍**(預設 BTCUSDT / ETHUSDT / BNBUSDT / SOLUSDT / DOGEUSDT / SHIBUSDT,interval 固定 `1m`) | Allowlist 控制的是 **startup REST backfill + 持續 WS stream** 的 (symbol, interval) 集合。**REST `GET /api/v1/klines` 不檢查 allowlist** —— 純 DB query,允許回到 allowlist 外 symbol 的舊資料(若 DB 曾累積過);DB 無資料才自然 `[]`。**前端不可把 allowlist 外的回應當「保證 []」**。`/quote subscribe` allowlist 外的 room:**可成功加入,但無 realtime tick**(emit 只從 upstream stream 出,allowlist 嚴格控制)。`trading_pairs` admin **僅控制 UI 顯示,與 ingestion 解耦** —— admin DELETE 某 symbol 不會關掉 stream;admin POST 非 allowlist symbol 也不會自動生 realtime。Allowlist 變動需 backend 重啟 |
| 2 | **`subscribe`/`unsubscribe` 無 ack** | 無錯誤回報、無確認時機;前端唯一 debug 依據 = server log + client-side counter。前端拍按鈕後要假設「已送出」,不要等 promise |
| 3 | **server 不 validate `SubscriptionPayload`**(無 `deny_unknown_fields`、無 trim) | `{ symbol, interval: "1m ", extra: "x" }`:`extra` 忽略;trailing space 讓 room 變 `"BTCUSDT:1m "`(與 emit room `"BTCUSDT:1m"` 不符)→ **silent fail**,server 不報錯。**前端必須自己 normalize**(`symbol.trim().toUpperCase()`、`interval` trim) |
| 4 | **`/api/v1/klines` query parse 失敗 = 400 plain-text**,非 `ErrorBody` JSON | 與 `/admin/*` 不一致;前端 fetch 後判 `content-type`,plain-text 路徑當 `invalid_param` 處理 |
| 5 | **`klines.open_time` snake_case vs `/quote.kline.openTime` camelCase** | 兩個 schema 都建,不要共用。Zod parse 邊界吸收差異 |
| 6 | **klinecharts 版本 pin** | CLAUDE.md 釘 `10.0.0-beta1`;官方 docs 站上目前 `beta2`。**前端 `package.json` 確認實際裝哪個**;beta1↔beta2 升版前對 Data Merge 行為(§6.2)做 regression smoke |
| 7 | **斷線重連不會自動 re-subscribe** | socket 重連後,server 端 room 已清;前端需在 `connect` event 重 emit `subscribe`(對應 leader 切換 / 網路 hiccup) |
| 8 | **`changed_by` 目前固定 `"admin:demo"`** | demo 無 per-admin 身分;AuditEntry 過濾 / 顯示時不要假設多種值 |
| 9 | **DB migration 是後端部署責任** | 後端 `main.rs` **無** `sqlx::migrate!`;新 schema 要先後端 `sqlx migrate run`,**Docker image build 成功 ≠ schema 已更新**。前端不需處理,但 staging / production 遇到 contract mismatch 時知道從哪找 |
| 10 | **`callUpdate.timestamp` 為 server clock** | 不可假設與 client clock 對齊;只當「event 順序 tag」用,不用來計算延遲 |

---

## 9. 變更管理

- 任何後端 wire contract 變動 → 先回 `ARCHITECTURE.md` §6.5 / §6.6 / §6.7 拍板(走 backend repo 的 `/backend-contract-review` skill),再同步更新本檔
- 本檔變動不需要前端 review,但前端 PR 引用本檔某段時請帶 anchor / commit SHA,方便比對
- contract drift(實際 wire 行為與本檔不符)= **後端 bug**,不在前端側 patch
