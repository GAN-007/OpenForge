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


export type KnowledgeNodeKind =
  | "file"
  | "function"
  | "method"
  | "class"
  | "struct"
  | "enum"
  | "trait"
  | "interface"
  | "type_alias"
  | "module"
  | "constant"
  | "variable";

export type KnowledgeEdgeKind =
  | "contains"
  | "imports"
  | "calls"
  | "references"
  | "changed_by";

export interface KnowledgeNode {
  id: string;
  name: string;
  kind: KnowledgeNodeKind;
  path: string;
  language: string;
  start_line: number;
  end_line: number;
  signature: string;
  sha256: string;
}

export interface KnowledgeEdge {
  from: string;
  to: string;
  kind: KnowledgeEdgeKind;
}

export interface GitFileHistory {
  commits: number;
  authors: Record<string, number>;
  latest_commit?: string | null;
  latest_unix_timestamp?: number | null;
}

export interface KnowledgeGraph {
  root: string;
  nodes: KnowledgeNode[];
  edges: KnowledgeEdge[];
  git_history: Record<string, GitFileHistory>;
  files_indexed: number;
}

export interface RecentEdit {
  file_path: string;
  before: string;
  after: string;
  cursor_line: number;
  cursor_column: number;
}

export interface DiagnosticContext {
  file_path: string;
  line: number;
  column: number;
  severity: string;
  message: string;
}

export interface EditRange {
  start_line: number;
  start_column: number;
  end_line: number;
  end_column: number;
}

export interface PredictedEdit {
  file_path: string;
  range: EditRange;
  new_text: string;
}

export interface CursorTarget {
  file_path: string;
  line: number;
  column: number;
}

export interface EditPredictionInput {
  run_id: string;
  file_path: string;
  language: string;
  prefix: string;
  suffix: string;
  recent_edits?: RecentEdit[];
  diagnostics?: DiagnosticContext[];
  semantic_context?: string;
  preferred_model_families?: string[];
  max_cost_usd: number;
  max_latency_ms?: number;
}

export interface EditPrediction {
  edits: PredictedEdit[];
  next_cursor?: CursorTarget | null;
  confidence: number;
  provider: string;
  model: string;
  latency_ms: number;
  cost_usd: number;
  cache_hit: boolean;
}

export interface LanguageServerConfig {
  name: string;
  program: string;
  args: string[];
  languages: string[];
  cwd?: string | null;
  environment: Record<string, string>;
  timeout_seconds: number;
  max_message_bytes: number;
  initialization_options: unknown;
}

export interface DebugAdapterConfig {
  name: string;
  adapter_id: string;
  program: string;
  args: string[];
  languages: string[];
  cwd?: string | null;
  environment: Record<string, string>;
  timeout_seconds: number;
  max_message_bytes: number;
}

export interface DapEvent {
  event: string;
  body: unknown;
}

export interface TerminalDescriptor {
  id: string;
  program: string;
  cwd: string;
  created_at: string;
  rows: number;
  cols: number;
}

export interface TerminalEvent {
  terminal_id: string;
  stream: string;
  data: string;
  timestamp: string;
}

export type WorkerStatus = "online" | "draining" | "offline";
export type WorkerJobStatus =
  | "queued"
  | "leased"
  | "completed"
  | "failed"
  | "cancelled";

export interface WorkerDescriptor {
  id: string;
  name: string;
  endpoint?: string | null;
  capabilities: string[];
  labels: string[];
  status: WorkerStatus;
  registered_at: string;
  last_heartbeat_at: string;
}

export interface DurableJob {
  id: string;
  run_id: string;
  task_id?: string | null;
  status: WorkerJobStatus;
  payload: unknown;
  required_capabilities: string[];
  attempts: number;
  max_attempts: number;
  created_at: string;
  updated_at: string;
}

export interface JobLease {
  job: DurableJob;
  worker_id: string;
  lease_token: string;
  leased_at: string;
  expires_at: string;
}

export interface JobCheckpoint {
  job_id: string;
  sequence: number;
  state: unknown;
  created_at: string;
}

export interface RichMemoryRecord {
  id: string;
  scope: string;
  key: string;
  value: unknown;
  provenance: string;
  confidence: number;
  expires_at?: string | null;
  repository_fingerprint?: string | null;
  embedding?: number[] | null;
  created_at: string;
  updated_at: string;
  last_accessed_at: string;
}

export interface RichMemoryInput {
  scope: string;
  key: string;
  value: unknown;
  provenance: string;
  confidence: number;
  expires_at?: string | null;
  repository_fingerprint?: string | null;
  embedding?: number[] | null;
}

export interface RichMemoryHit {
  record: RichMemoryRecord;
  lexical_score: number;
  semantic_score?: number | null;
  stale: boolean;
  combined_score: number;
}

export type TeamRole =
  | "viewer"
  | "reviewer"
  | "developer"
  | "maintainer"
  | "admin"
  | "owner";

export interface TeamIdentity {
  id: string;
  subject: string;
  email?: string | null;
  display_name?: string | null;
  provider: string;
  created_at: string;
}

export interface TeamWorkspace {
  id: string;
  slug: string;
  name: string;
  created_at: string;
}

export interface WorkspaceRepository {
  id: string;
  workspace_id: string;
  name: string;
  canonical_path: string;
  created_at: string;
}

export type ThreadStatus =
  | "active"
  | "paused"
  | "taken_over"
  | "completed"
  | "cancelled";

export interface AgentThread {
  id: string;
  run_id: string;
  task_id?: string | null;
  title: string;
  status: ThreadStatus;
  owner_subject?: string | null;
  created_at: string;
  updated_at: string;
}

export interface ThreadMessage {
  id: string;
  thread_id: string;
  sequence: number;
  author_kind: string;
  author_id: string;
  message_type: string;
  content: unknown;
  created_at: string;
}

export interface QueuedInstruction {
  id: string;
  thread_id: string;
  content: string;
  submitted_by: string;
  consumed_at?: string | null;
  created_at: string;
}

export type ApprovalStatus =
  | "pending"
  | "approved_once"
  | "approved_always"
  | "denied";

export interface ApprovalRequest {
  id: string;
  thread_id: string;
  capability: string;
  subject: string;
  reason: string;
  status: ApprovalStatus;
  decided_by?: string | null;
  created_at: string;
  decided_at?: string | null;
}

export interface Presence {
  workspace_id: string;
  subject: string;
  surface: string;
  resource?: string | null;
  last_seen_at: string;
}

export interface ReviewComment {
  id: string;
  workspace_id: string;
  run_id?: string | null;
  task_id?: string | null;
  file_path?: string | null;
  line?: number | null;
  author: string;
  body: string;
  resolved: boolean;
  created_at: string;
  resolved_at?: string | null;
}

export interface LoadedPlugin {
  manifest: {
    schema: string;
    id: string;
    version: string;
    runtime: "wasm" | "process" | "mcp";
    entrypoint: string;
    api: string;
    capabilities: Record<string, string[]>;
    contributes: Record<string, string[]>;
  };
  root: string;
  entrypoint_sha256: string;
}

export interface PluginInvocation {
  method: string;
  params?: unknown;
  requested_capabilities?: Record<string, string[]>;
  timeout_seconds?: number;
}

export type DebugPhase =
  | "reproduce"
  | "hypothesize"
  | "instrument"
  | "observe"
  | "select_fix"
  | "apply_fix"
  | "verify"
  | "cleanup"
  | "completed"
  | "failed";

export interface DebugHypothesis {
  id: string;
  statement: string;
  confidence: number;
  evidence_for: string[];
  evidence_against: string[];
}

export interface DebugObservation {
  id: string;
  kind: string;
  source: string;
  data: unknown;
  created_at: string;
}

export interface DebugInstrumentation {
  id: string;
  description: string;
  file: string;
  reversible_patch: string;
  applied: boolean;
  removed: boolean;
}

export interface DebugSession {
  id: string;
  run_id: string;
  task_id?: string | null;
  issue: string;
  phase: DebugPhase;
  hypotheses: DebugHypothesis[];
  observations: DebugObservation[];
  instrumentation: DebugInstrumentation[];
  selected_hypothesis?: string | null;
  fix_summary?: string | null;
  verification_summary?: string | null;
  created_at: string;
  updated_at: string;
}

export interface WorkspaceFile {
  path: string;
  content: string;
  bytes: number;
}

export interface GitStatusEntry {
  index: string;
  worktree: string;
  path: string;
  original_path?: string | null;
}

export interface GitStatusResult {
  branch?: string | null;
  entries: GitStatusEntry[];
}

export interface GitDiffResult {
  diff: string;
}
