use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

pub const PROTOCOL_VERSION: &str = "openforge.protocol.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    Observe,
    Suggest,
    Edit,
    Execute,
    Autonomous,
}

impl Default for AutonomyLevel {
    fn default() -> Self { Self::Suggest }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Planning,
    AwaitingApproval,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Ready,
    Running,
    AwaitingApproval,
    Reviewing,
    Completed,
    Failed,
    Cancelled,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    pub currency: String,
    pub soft_limit: Option<f64>,
    pub hard_limit: f64,
    pub spent: f64,
}

impl Budget {
    pub fn remaining(&self) -> f64 { (self.hard_limit - self.spent).max(0.0) }
    pub fn reserve(&self, amount: f64) -> bool { amount >= 0.0 && amount <= self.remaining() }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: Uuid,
    pub project_id: Uuid,
    pub objective: String,
    pub base_sha: String,
    pub status: RunStatus,
    pub autonomy: AutonomyLevel,
    pub budget: Budget,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCommand {
    pub argv: Vec<String>,
    #[serde(default)]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBudget {
    pub max_usd: f64,
    pub max_model_calls: u32,
    pub max_tool_calls: u32,
    pub max_wall_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: Uuid,
    pub run_id: Uuid,
    pub title: String,
    pub description: String,
    pub role: String,
    #[serde(default)]
    pub dependencies: Vec<Uuid>,
    #[serde(default)]
    pub required_reviews: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCommand>,
    pub status: TaskStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub budget: TaskBudget,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: String,
    pub role: String,
    pub system_prompt: String,
    pub autonomy: AutonomyLevel,
    pub model_policy: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub denied_tools: Vec<String>,
    pub max_iterations: u32,
    pub max_delegation_depth: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: Uuid,
    pub sequence: i64,
    pub run_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    pub actor: Actor,
    pub event_type: String,
    pub payload: Value,
    pub previous_event_hash: Option<String>,
    pub event_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequirements {
    #[serde(default)]
    pub task_class: String,
    pub context_tokens: u32,
    pub requires_tools: bool,
    pub requires_vision: bool,
    pub requires_structured_output: bool,
    pub max_cost_usd: f64,
    pub max_latency_ms: Option<u64>,
    pub data_classification: DataClassification,
    #[serde(default)]
    pub preferred_model_families: Vec<String>,
    #[serde(default)]
    pub excluded_model_families: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DataClassification {
    Public,
    Internal,
    Confidential,
    Restricted,
}

impl Default for DataClassification {
    fn default() -> Self { Self::Internal }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub provider: String,
    pub model: String,
    pub family: String,
    pub context_tokens: u32,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_structured_output: bool,
    pub input_usd_per_million: f64,
    pub output_usd_per_million: f64,
    pub latency_score: f64,
    pub quality_score: f64,
    pub privacy_score: f64,
    pub max_data_classification: DataClassification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub invocation_id: Uuid,
    pub run_id: Uuid,
    pub task_id: Option<Uuid>,
    pub messages: Vec<ChatMessage>,
    pub requirements: ModelRequirements,
    pub temperature: f32,
    pub max_output_tokens: u32,
    pub response_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub provider: String,
    pub model: String,
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub latency_ms: u64,
    pub cost_usd: f64,
    pub provider_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySet {
    pub protocol_version: String,
    pub server_version: String,
    pub capabilities: BTreeMap<String, bool>,
}
