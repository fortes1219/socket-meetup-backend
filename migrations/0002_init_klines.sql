-- V002: klines lightweight time-series schema
-- 參考 ARCHITECTURE.md §9 step 7 + CLAUDE.md(K 線走輕量時序表,不套 §6.6 audit)
--
-- 設計差異(對比 §6.6 trading_pairs):
--   ▸ 不套 audit pattern(K 線是上游事實,不是業務 mutation)
--   ▸ 不設 surrogate id,直接 composite PK (symbol, interval, open_time)
--   ▸ INSERT 用 ON CONFLICT DO UPDATE,容忍 WS 重連 replay
--   ▸ 跟 trading_pairs 鬆耦合(symbol 字串關聯,不設 FK)
--
-- 不做:partitioning / TimescaleDB(demo 量小不需要,production scaling 再加)

CREATE TABLE klines (
    symbol       TEXT NOT NULL,
    interval     TEXT NOT NULL,                     -- '1m'|'5m'|'15m'|'1h'|'4h'|'1d' 等
    open_time    TIMESTAMPTZ NOT NULL,
    close_time   TIMESTAMPTZ NOT NULL,
    open         NUMERIC(20, 8) NOT NULL,
    high         NUMERIC(20, 8) NOT NULL,
    low          NUMERIC(20, 8) NOT NULL,
    close        NUMERIC(20, 8) NOT NULL,
    volume       NUMERIC(20, 8) NOT NULL,           -- base asset volume
    trades_count INTEGER        NOT NULL DEFAULT 0,
    PRIMARY KEY (symbol, interval, open_time)
);

-- ─── Index 策略 ───
-- getBars 熱路徑:WHERE symbol=? AND interval=? ORDER BY open_time DESC LIMIT N
-- 已被 composite PK (symbol, interval, open_time) 的 B-tree 完全覆蓋,
-- PG 對 DESC scan 跟 ASC 同速度(reverse scan),不需要額外 index。
-- 等真的 query plan 顯示 issue 再加。
