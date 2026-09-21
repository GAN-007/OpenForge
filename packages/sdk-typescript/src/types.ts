export type AutonomyLevel =
  | "observe"
  | "suggest"
  | "edit"
  | "execute"
  | "autonomous";

export type RunStatus =
  | "planning"
  | "awaiting_approval"
  | "running"
  | "paused"
  | "completed"
  | "failed"
  | "cancelled";

export type TaskStatus =
  | "pending"
  | "ready"
  | "running"
  | "awaiting_approval"
  | "reviewing"
  | "completed"
  | "failed"
  | "cancelled"
  | "blocked";

export interface Budget {
  currency: string;
  soft_limit?: number | null;
  hard_limit: number;
  spent: number;
}

export interface Run {
  id: string;
  project_id: string;
  objective: string;
  base_sha: string;
  status: RunStatus;
  autonomy: AutonomyLevel;
  budget: Budget;
  created_at: string;
  updated_at: string;
}

export interface TaskNode {
  id: string;
  run_id: string;
  title: string;
  description: string;
  role: string;
  dependencies: string[];
  required_reviews: string[];
  status: TaskStatus;
  attempts: number;
  max_attempts: number;
  budget: {
    max_usd: number;
    max_model_calls: number;
    max_tool_calls: number;
    max_wall_seconds: number;
  };
  requirements: {
    capabilities: Array<
      | "filesystem_read"
      | "filesystem_write"
      | "process"
      | "network"
      | "mcp"
      | "acp"
      | "database_read"
      | "database_write"
      | "secrets"
      | "cloud_read"
      | "cloud_write"
      | "deployment"
      | "browser"
      | "git_read"
      | "git_write"
    >;
    resources: {
      cpu_cores: number;
      memory_mb: number;
      disk_mb: number;
      pids: number;
      wall_seconds: number;
      max_stdout_bytes: number;
      max_stderr_bytes: number;
    };
    preferred_languages: string[];
    required_reviews: string[];
    exclusive_resources: string[];
  };
  acceptance: Array<{ argv: string[]; timeout_seconds: number }>;
  created_at: string;
  updated_at: string;
}

export interface EventEnvelope {
  event_id: string;
  sequence: number;
  run_id?: string | null;
  task_id?: string | null;
  timestamp: string;
  actor: { kind: string; id: string };
  event_type: string;
  payload: unknown;
  previous_event_hash?: string | null;
  event_hash: string;
}

export interface CapabilitySet {
  protocol_version: string;
  server_version: string;
  capabilities: Record<string, boolean>;
}

export interface FileRecord {
  path: string;
  bytes: number;
  sha256: string;
  language: string;
  lines: number;
  is_test: boolean;
}

export interface RepositoryIndex {
  root: string;
  fingerprint: string;
  files: FileRecord[];
  language_counts: Record<string, number>;
  total_bytes: number;
  total_lines: number;
}

export interface SearchHit {
  path: string;
  score: number;
  matched_terms: string[];
  line: number;
  snippet: string;
}

export type SymbolKind =
  | "function"
  | "method"
  | "struct"
  | "class"
  | "enum"
  | "trait"
  | "interface"
  | "type_alias"
  | "module"
  | "constant"
  | "variable";

export interface SymbolRecord {
  id: string;
  name: string;
  kind: SymbolKind;
  path: string;
  line: number;
  signature: string;
  language: string;
}

export interface SymbolEdge {
  from: string;
  to: string;
  kind: "imports" | "references" | "calls";
}

export interface SymbolGraph {
  symbols: SymbolRecord[];
  edges: SymbolEdge[];
  files_indexed: number;
}

export interface ArtifactDescriptor {
  digest: string;
  bytes: number;
  media_type: string;
  source: string;
  created_at: string;
  metadata: Record<string, unknown>;
}

export interface EventIntegrityReport {
  valid: boolean;
  verified_events: number;
  first_sequence?: number | null;
  last_sequence?: number | null;
  terminal_hash?: string | null;
  violations: string[];
}

export interface HistogramSnapshot {
  count: number;
  sum: number;
  min?: number | null;
  max?: number | null;
  average?: number | null;
}

export interface TelemetryEvent {
  timestamp: string;
  name: string;
  attributes: Record<string, unknown>;
}

export interface TelemetrySnapshot {
  generated_at: string;
  counters: Record<string, number>;
  gauges: Record<string, number>;
  histograms: Record<string, HistogramSnapshot>;
  recent_events: TelemetryEvent[];
}

export interface SecretLeaseDescriptor {
  id: string;
  secret_name: string;
  issued_at: string;
  expires_at: string;
  audience: string;
  renewable: boolean;
}

export interface BudgetLimits {
  per_call: number;
  per_task: number;
  per_run: number;
  daily: number;
}

export interface BudgetSnapshot {
  run_id: string;
  task_spent_usd: number;
  run_spent_usd: number;
  daily_spent_usd: number;
  reserved_usd: number;
  daily_reserved_usd: number;
  limits: BudgetLimits;
}

export interface PluginCapabilities {
  filesystem: string[];
  network: string[];
  secrets: string[];
  database: string[];
  shell: string[];
  mcp: string[];
  acp: string[];
  cloud: string[];
  deployment: string[];
  browser: string[];
  git: string[];
}

export interface PluginManifest {
  schema: "openforge.plugin/v2";
  id: string;
  version: string;
  runtime: "wasm" | "process" | "mcp";
  entrypoint: string;
  api: string;
  capabilities: PluginCapabilities;
  contributes: Record<string, string[]>;
}

export interface PluginCapabilityDeclaration {
  domain: string;
  values: string[];
}


export interface ModelSpec {
  provider: string;
  model: string;
  family: string;
  context_tokens: number;
  supports_tools: boolean;
  supports_vision: boolean;
  supports_structured_output: boolean;
  input_usd_per_million: number;
  output_usd_per_million: number;
  latency_score: number;
  quality_score: number;
  privacy_score: number;
  max_data_classification: "PUBLIC" | "INTERNAL" | "CONFIDENTIAL" | "RESTRICTED";
}

export interface GatewayStatus {
  connected: boolean;
  base_url: string;
  model: string;
  credential_storage: "daemon_memory" | "user_config_file";
  credential_persisted?: boolean;
  pricing: "gateway_reported_or_unpriced";
}
