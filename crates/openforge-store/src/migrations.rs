pub(crate) const RUNTIME_INTEGRATION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS secret_leases (
    lease_id TEXT PRIMARY KEY,
    secret_name TEXT NOT NULL,
    audience TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_secret_leases_issued
    ON secret_leases(issued_at DESC);

CREATE TABLE IF NOT EXISTS secret_lease_revocations (
    lease_id TEXT PRIMARY KEY REFERENCES secret_leases(lease_id) ON DELETE CASCADE,
    revoked_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS acp_processes (
    process_id TEXT PRIMARY KEY,
    program TEXT NOT NULL,
    started_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_acp_processes_started
    ON acp_processes(started_at DESC);

CREATE TABLE IF NOT EXISTS budget_limits (
    run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    per_call REAL NOT NULL CHECK(per_call >= 0),
    per_task REAL NOT NULL CHECK(per_task >= 0),
    per_run REAL NOT NULL CHECK(per_run >= 0),
    daily REAL NOT NULL CHECK(daily >= 0)
);

CREATE TABLE IF NOT EXISTS budget_reservations (
    reservation_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    estimated_usd REAL NOT NULL CHECK(estimated_usd >= 0),
    actual_usd REAL CHECK(actual_usd IS NULL OR actual_usd >= 0),
    created_at TEXT NOT NULL,
    settled_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_budget_reservations_run
    ON budget_reservations(run_id, created_at DESC);
"#;
