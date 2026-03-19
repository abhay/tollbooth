CREATE TABLE IF NOT EXISTS consumed_signatures (
    signature TEXT PRIMARY KEY,
    protocol TEXT NOT NULL,
    consumed_at INTEGER NOT NULL,
    amount TEXT,
    payer TEXT,
    receipt_json TEXT
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    bearer_hash TEXT NOT NULL,
    deposit_amount INTEGER NOT NULL,
    spent INTEGER NOT NULL DEFAULT 0,
    refund_address TEXT NOT NULL,
    mint TEXT NOT NULL,
    decimals INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    refund_signature TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    swept_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_status_swept ON sessions(status, swept_at);

CREATE TABLE IF NOT EXISTS webhook_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_attempt_at INTEGER,
    delivered INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS metrics_1m (
    ts INTEGER NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (ts, metric)
);

CREATE TABLE IF NOT EXISTS metrics_1h (
    ts INTEGER NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (ts, metric)
);

CREATE TABLE IF NOT EXISTS metrics_1d (
    ts INTEGER NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (ts, metric)
);

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO meta (key, value) VALUES ('last_rollup_1h', '0');
INSERT OR IGNORE INTO meta (key, value) VALUES ('last_rollup_1d', '0');
