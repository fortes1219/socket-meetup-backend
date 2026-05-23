-- V001: trading_pairs schema (state + append-only audit)
-- 參考 ARCHITECTURE.md §6.6

-- ─── 當前狀態(白名單:平台允許的幣安交易對)───
CREATE TABLE trading_pairs (
    id            UUID PRIMARY KEY,
    symbol        TEXT NOT NULL UNIQUE,
    base_asset    TEXT NOT NULL,
    quote_asset   TEXT NOT NULL,
    enabled       BOOLEAN NOT NULL DEFAULT false,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at    TIMESTAMPTZ                       -- soft delete,絕不 hard
);

-- ─── Append-only audit(只記動作,不存 snapshot)───
CREATE TABLE trading_pair_audit (
    audit_id        UUID PRIMARY KEY,
    trading_pair_id UUID NOT NULL REFERENCES trading_pairs(id),
    action          TEXT NOT NULL,                  -- 'added'|'enabled'|'disabled'|'removed'|'reordered'
    changed_by      TEXT NOT NULL DEFAULT 'admin:demo',
    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ─── 物理層保證 append-only(不是君子協定) ───
CREATE RULE no_update_audit AS ON UPDATE TO trading_pair_audit DO INSTEAD NOTHING;
CREATE RULE no_delete_audit AS ON DELETE TO trading_pair_audit DO INSTEAD NOTHING;

-- ─── Indexes ───
-- 熱讀路徑:前端拿可見清單(partial index 省 50%+ 空間)
CREATE INDEX idx_pairs_visible
    ON trading_pairs(display_order, symbol)
    WHERE deleted_at IS NULL AND enabled = true;

-- 後台:列出所有未刪除(含 disabled)
CREATE INDEX idx_pairs_managed
    ON trading_pairs(symbol)
    WHERE deleted_at IS NULL;

-- Audit 時間軸查詢
CREATE INDEX idx_audit_time
    ON trading_pair_audit(occurred_at DESC);

CREATE INDEX idx_audit_pair_time
    ON trading_pair_audit(trading_pair_id, occurred_at DESC);
