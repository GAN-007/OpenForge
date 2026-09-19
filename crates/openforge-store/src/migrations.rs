pub(crate) const SCHEMA: &str = r#"
            CREATE TABLE IF NOT EXISTS runs (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL,
              objective TEXT NOT NULL,
              base_sha TEXT NOT NULL,
              status TEXT NOT NULL,
              autonomy TEXT NOT NULL,
              budget_json TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
              id TEXT PRIMARY KEY,
              run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
              task_json TEXT NOT NULL,
              status TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_run ON tasks(run_id);

            CREATE TABLE IF NOT EXISTS events (
              sequence INTEGER PRIMARY KEY AUTOINCREMENT,
              event_id TEXT NOT NULL UNIQUE,
              run_id TEXT,
              task_id TEXT,
              timestamp TEXT NOT NULL,
              actor_json TEXT NOT NULL,
              event_type TEXT NOT NULL,
              payload_json TEXT NOT NULL,
              previous_event_hash TEXT,
              event_hash TEXT NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_events_run_sequence
              ON events(run_id, sequence);

            CREATE TABLE IF NOT EXISTS cost_ledger (
              id TEXT PRIMARY KEY,
              run_id TEXT NOT NULL,
              task_id TEXT,
              agent_id TEXT,
              provider TEXT NOT NULL,
              model TEXT NOT NULL,
              amount_usd REAL NOT NULL CHECK(amount_usd >= 0),
              input_tokens INTEGER NOT NULL DEFAULT 0,
              output_tokens INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_cost_run ON cost_ledger(run_id);

            CREATE TABLE IF NOT EXISTS memory (
              id TEXT PRIMARY KEY,
              scope TEXT NOT NULL,
              project_id TEXT,
              repository_id TEXT,
              key TEXT NOT NULL,
              value_json TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              UNIQUE(scope, project_id, repository_id, key)
            );
            CREATE TABLE IF NOT EXISTS secret_leases (
              lease_id TEXT PRIMARY KEY,
              secret_name TEXT NOT NULL,
              audience TEXT NOT NULL,
              issued_at TEXT NOT NULL,
              expires_at TEXT NOT NULL,
              renewable INTEGER NOT NULL DEFAULT 0,
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
              started_at TEXT NOT NULL,
              closed_at TEXT
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
