use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

pub mod ids;
pub use ids::{
    AgentInstanceId, ApprovalId, EventId, ModelInvocationId, ProjectId, RunId, SecretLeaseId,
    SessionId, TaskId, ToolInvocationId,
};

pub const PROTOCOL_VERSION: &str = "openforge.protocol.v2";

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
    fn default() -> Self {
        Self::Suggest
    }
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
    pub fn remaining(&self) -> f64 {
        (self.hard_limit - self.spent).max(0.0)
    }
    pub fn reserve(&self, amount: f64) -> bool {
        amount >= 0.0 && amount <= self.remaining()
    }
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
    #[serde(default)]
    pub requirements: TaskRequirements,
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
    fn default() -> Self {
        Self::Internal
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu_cores: f32,
    pub memory_mb: u64,
    pub disk_mb: u64,
    pub pids: u32,
    pub wall_seconds: u64,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_cores: 2.0,
            memory_mb: 4096,
            disk_mb: 20_480,
            pids: 256,
            wall_seconds: 2700,
            max_stdout_bytes: 8 * 1024 * 1024,
            max_stderr_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskRequirements {
    #[serde(default)]
    pub capabilities: Vec<CapabilityDomain>,
    #[serde(default)]
    pub resources: ResourceLimits,
    #[serde(default)]
    pub preferred_languages: Vec<String>,
    #[serde(default)]
    pub required_reviews: Vec<String>,
    #[serde(default)]
    pub exclusive_resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDomain {
    FilesystemRead,
    FilesystemWrite,
    Process,
    Network,
    Mcp,
    Acp,
    DatabaseRead,
    DatabaseWrite,
    Secrets,
    CloudRead,
    CloudWrite,
    Deployment,
    Browser,
    GitRead,
    GitWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSecurityProfile {
    pub read_only_root: bool,
    pub workspace_read_only: bool,
    pub no_new_privileges: bool,
    pub drop_all_capabilities: bool,
    pub seccomp: bool,
    pub seccomp_profile: Option<String>,
    pub apparmor_profile: Option<String>,
    pub require_rootless: bool,
    pub run_as_non_root: bool,
    pub runtime: Option<String>,
    pub network_mode: String,
    pub network_proxy: Option<String>,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub denied_cidrs: Vec<String>,
}

impl Default for SandboxSecurityProfile {
    fn default() -> Self {
        Self {
            read_only_root: true,
            workspace_read_only: false,
            no_new_privileges: true,
            drop_all_capabilities: true,
            seccomp: true,
            seccomp_profile: None,
            apparmor_profile: None,
            require_rootless: false,
            run_as_non_root: true,
            runtime: None,
            network_mode: "none".into(),
            network_proxy: None,
            allowed_hosts: Vec::new(),
            denied_cidrs: vec![
                "127.0.0.0/8".into(),
                "169.254.0.0/16".into(),
                "10.0.0.0/8".into(),
                "172.16.0.0/12".into(),
                "192.168.0.0/16".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretLeaseDescriptor {
    pub id: SecretLeaseId,
    pub secret_name: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub audience: String,
    pub renewable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub sha256: String,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewGate {
    pub role: String,
    pub required: bool,
    pub independent_model_family: bool,
    pub blocking_severities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionCheckpoint {
    pub run_id: Uuid,
    pub task_id: Option<Uuid>,
    pub sequence: i64,
    pub integration_sha: String,
    pub created_at: DateTime<Utc>,
    pub state: Value,
}
