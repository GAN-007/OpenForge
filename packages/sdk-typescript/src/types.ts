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


export interface ModelRuntimeSpec {
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
  max_data_classification: string;
}

export interface SharedModelSettings {
  provider_name: string;
  kind: "openai-compatible" | "anthropic" | "gemini" | "bedrock-aws-cli";
  base_url: string;
  model: string;
  family: string;
  context_tokens: number;
  tools: boolean;
  vision: boolean;
  structured_output: boolean;
  input_usd_per_million: number;
  output_usd_per_million: number;
  latency_score: number;
  quality_score: number;
  privacy_score: number;
  region?: string | null;
  header_names: string[];
  api_key_configured: boolean;
}

export interface ModelSettingsState {
  configured: boolean;
  settings?: SharedModelSettings | null;
  active_providers: string[];
  models: ModelRuntimeSpec[];
}

export interface ModelSettingsInput {
  provider_name?: string;
  kind?: "openai-compatible" | "anthropic" | "gemini" | "bedrock-aws-cli";
  base_url?: string;
  model: string;
  family?: string;
  context_tokens?: number;
  tools?: boolean;
  vision?: boolean;
  structured_output?: boolean;
  input_usd_per_million?: number;
  output_usd_per_million?: number;
  latency_score?: number;
  quality_score?: number;
  privacy_score?: number;
  region?: string;
  headers?: Record<string, string>;
  api_key?: string;
  retain_existing_api_key?: boolean;
}
