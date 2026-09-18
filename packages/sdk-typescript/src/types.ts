export type AutonomyLevel = "observe" | "suggest" | "edit" | "execute" | "autonomous";
export type RunStatus = "planning" | "awaiting_approval" | "running" | "paused" | "completed" | "failed" | "cancelled";
export type TaskStatus = "pending" | "ready" | "running" | "awaiting_approval" | "reviewing" | "completed" | "failed" | "cancelled" | "blocked";

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
  budget: {max_usd:number;max_model_calls:number;max_tool_calls:number;max_wall_seconds:number};
  created_at: string;
  updated_at: string;
}
export interface EventEnvelope {
  event_id: string;
  sequence: number;
  run_id?: string | null;
  task_id?: string | null;
  timestamp: string;
  actor: {kind:string;id:string};
  event_type: string;
  payload: unknown;
  previous_event_hash?: string | null;
  event_hash: string;
}
export interface CapabilitySet {
  protocol_version: string;
  server_version: string;
  capabilities: Record<string,boolean>;
}
