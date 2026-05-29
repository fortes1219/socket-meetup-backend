-- V003: 讓 §6.6 contract 宣稱的 enum / >= 0 response schema 有資料層保證
-- 參考 ARCHITECTURE.md §6.6(A-3.3 schema 變更);不改已落地的 0001。

-- audit action 限定為合法 enum(AuditEntry.action 非任意 TEXT)
ALTER TABLE trading_pair_audit
    ADD CONSTRAINT chk_trading_pair_audit_action
    CHECK (action IN ('added', 'enabled', 'disabled', 'removed', 'reordered'));

-- display_order 不可為負(後台顯示順序,無負值語意)
ALTER TABLE trading_pairs
    ADD CONSTRAINT chk_trading_pairs_display_order
    CHECK (display_order >= 0);
