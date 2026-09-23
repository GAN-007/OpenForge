import type {
  ArtifactDescriptor,
  BudgetSnapshot,
  CapabilitySet,
  EventEnvelope,
  EventIntegrityReport,
  PluginCapabilityDeclaration,
  PluginManifest,
  ModelSpec,
  ModelPreflight,
  OllamaModelList,
  OllamaPullResult,
  GatewayStatus,
  RepositoryIndex,
  Run,
  SearchHit,
  SecretLeaseDescriptor,
  SymbolGraph,
  SymbolRecord,
  TaskNode,
  TelemetrySnapshot,
} from "./types.js";

function structuredMcpResult<T>(result: {
  content: unknown[];
  isError: boolean;
  structuredContent?: unknown;
}): T {
  if (result.isError) {
    throw new Error("Ollama MCP tool reported an error");
  }
  if (result.structuredContent && typeof result.structuredContent === "object") {
    return result.structuredContent as T;
  }
  for (const item of result.content) {
    if (
      item &&
      typeof item === "object" &&
      "type" in item &&
      "text" in item &&
      (item as { type?: unknown }).type === "text" &&
      typeof (item as { text?: unknown }).text === "string"
    ) {
      return JSON.parse((item as { text: string }).text) as T;
    }
  }
  throw new Error("Ollama MCP tool returned no structured result");
}

interface RpcResponse<T> {
  jsonrpc: "2.0";
  id: number;
  result?: T;
  error?: { code: number; message: string; data?: unknown };
}

export class OpenForgeClient {
  private nextId = 1;
  readonly apiToken: string | undefined;

  constructor(
    readonly baseUrl = "http://127.0.0.1:8765",
    apiToken?: string,
  ) {
    this.apiToken = apiToken;
  }

  async rpc<T>(
    method: string,
    params: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<T> {
    const id = this.nextId++;
    const headers: Record<string, string> = {
      "content-type": "application/json",
    };
    if (this.apiToken) {
      headers.authorization = "Bearer " + this.apiToken;
    }

    const response = await fetch(this.baseUrl + "/v1/rpc", {
      method: "POST",
      headers,
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
      signal: signal ?? null,
    });
    const body = (await response.json()) as RpcResponse<T>;
    if (!body || typeof body !== "object" || body.jsonrpc !== "2.0" || body.id !== id) {
      throw new Error("OpenForge RPC returned an invalid or mismatched response");
    }
    if (!response.ok || body.error) {
      throw new Error(
        body.error?.message ?? "OpenForge RPC failed: " + response.status,
      );
    }
    if (body.result === undefined) {
      throw new Error("OpenForge RPC returned no result");
    }
    return body.result;
  }

  gatewayStatus() { return this.rpc<GatewayStatus>("gateway/status"); }

  connectGateway(apiKey: string) {
    return this.rpc<GatewayStatus>("gateway/connect", { api_key: apiKey });
  }

  disconnectGateway() { return this.rpc<GatewayStatus>("gateway/disconnect"); }

  initialize() {
    return this.rpc<CapabilitySet>("initialize");
  }

  createRun(params: {
    repo: string;
    objective: string;
    autonomy?: string;
    budget_usd?: number;
  }) {
    return this.rpc<Run>("run/create", params);
  }

  planRun(repo: string, runId: string) {
    return this.rpc<TaskNode[]>("run/plan", {
      repo,
      run_id: runId,
    });
  }

  executeRun(params: {
    repo: string;
    run_id: string;
    policy_path?: string;
    docker?: boolean;
    runner_backend?: "local" | "docker" | "kubernetes";
  }) {
    return this.rpc<{ integration_branch: string }>(
      "run/execute",
      params,
    );
  }

  listRuns(limit = 100, offset = 0) {
    return this.rpc<Run[]>("run/list", { limit, offset });
  }

  /** Stream durable audit events. Reconnect using the last yielded sequence. */
  async *streamEvents(runId: string, afterSequence = 0, signal?: AbortSignal): AsyncGenerator<EventEnvelope> {
    const headers: Record<string, string> = { accept: "text/event-stream" };
    if (this.apiToken) headers.authorization = "Bearer " + this.apiToken;
    const response = await fetch(
      this.baseUrl.replace(/\/$/, "") + "/v1/runs/" + encodeURIComponent(runId) +
        "/events/stream?after_sequence=" + afterSequence,
      { headers, signal: signal ?? null },
    );
    if (!response.ok || !response.body) throw new Error("OpenForge event stream failed: " + response.status);
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let eventType = "";
    let data: string[] = [];
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let newline: number;
        while ((newline = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, newline).replace(/\r$/, "");
          buffer = buffer.slice(newline + 1);
          if (line === "") {
            if (eventType === "error") throw new Error(data.join("\n"));
            if (data.length && eventType === "audit") yield JSON.parse(data.join("\n")) as EventEnvelope;
            eventType = "";
            data = [];
          } else if (line.startsWith("event:")) {
            eventType = line.slice(6).replace(/^ /, "");
          } else if (line.startsWith("data:")) {
            data.push(line.slice(5).replace(/^ /, ""));
          }
        }
      }
    } finally {
      await reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }
  }

  getRun(runId: string) {
    return this.rpc<Run | null>("run/get", { run_id: runId });
  }

  listTasks(runId: string) {
    return this.rpc<TaskNode[]>("task/list", { run_id: runId });
  }

  listEvents(runId: string, afterSequence = 0, limit = 500) {
    return this.rpc<EventEnvelope[]>("event/list", {
      run_id: runId,
      after_sequence: afterSequence,
      limit,
    });
  }

  verifyEvents() {
    return this.rpc<EventIntegrityReport>("event/verify");
  }

  getBudget(runId: string) {
    return this.rpc<{ run_id: string; spent_usd: number }>(
      "budget/get",
      { run_id: runId },
    );
  }

  searchMemory(query: string, scope?: string, limit = 100) {
    return this.rpc<unknown[]>("memory/search", {
      query,
      scope,
      limit,
    });
  }

  putMemory(params: {
    scope: string;
    key: string;
    value: unknown;
    project_id?: string;
    repository_id?: string;
  }) {
    return this.rpc<{ ok: boolean }>("memory/put", params);
  }

  deleteMemory(key: string, scope?: string) {
    return this.rpc<{ deleted: number }>("memory/delete", {
      key,
      scope,
    });
  }

  completion(
    params: {
      run_id: string;
      file_path: string;
      language: string;
      prefix: string;
      suffix: string;
      max_output_tokens?: number;
      max_cost_usd?: number;
    },
    signal?: AbortSignal,
  ) {
    return this.rpc<{ text: string }>(
      "completion/request",
      params,
      signal,
    );
  }

  repositoryIndex(repo: string) {
    return this.rpc<RepositoryIndex>("repository/index", { repo });
  }

  search(repo: string, query: string, limit = 25) {
    return this.rpc<{
      stats: Record<string, number>;
      hits: SearchHit[];
    }>("search/query", { repo, query, limit });
  }

  symbols(repo: string, query: string, limit = 25) {
    return this.rpc<{
      stats: Record<string, number>;
      symbols: SymbolRecord[];
    }>("symbols/query", { repo, query, limit });
  }

  symbolGraph(repo: string) {
    return this.rpc<SymbolGraph>("symbols/graph", { repo });
  }

  putArtifact(params: {
    base64: string;
    media_type?: string;
    source?: string;
    metadata?: Record<string, unknown>;
  }) {
    return this.rpc<ArtifactDescriptor>("artifact/put", params);
  }

  getArtifact(sha256: string) {
    return this.rpc<{
      descriptor: ArtifactDescriptor;
      base64: string;
    }>("artifact/get", { sha256 });
  }

  artifactDescriptor(sha256: string) {
    return this.rpc<ArtifactDescriptor>("artifact/descriptor", {
      sha256,
    });
  }

  deleteArtifact(sha256: string) {
    return this.rpc<{ deleted: boolean }>("artifact/delete", {
      sha256,
    });
  }

  evaluatePolicy(params: {
    policy_path: string;
    capability: string;
    subject?: string;
    argv?: string[];
  }) {
    return this.rpc<{ decision: "allow" | "ask" | "deny" }>(
      "policy/evaluate",
      params,
    );
  }

  telemetry() {
    return this.rpc<TelemetrySnapshot>("telemetry/snapshot");
  }

  listModels() {
    return this.rpc<ModelSpec[]>("model/list");
  }

  providers() {
    return this.rpc<{ providers: string[] }>("model/providers");
  }

  preflightModels() {
    return this.rpc<ModelPreflight[]>("model/preflight");
  }

  async ollamaListModels(
    policyPath = "config/policies/development.yaml",
  ): Promise<OllamaModelList> {
    return structuredMcpResult<OllamaModelList>(
      await this.mcpCallTool("ollama", "list_models", {}, policyPath),
    );
  }

  async ollamaPullModel(
    model: string,
    policyPath = "config/policies/development.yaml",
  ): Promise<OllamaPullResult> {
    return structuredMcpResult<OllamaPullResult>(
      await this.mcpCallTool("ollama", "pull_model", { model }, policyPath),
    );
  }

  async ollamaShowModel(
    model: string,
    policyPath = "config/policies/development.yaml",
  ): Promise<Record<string, unknown>> {
    return structuredMcpResult<Record<string, unknown>>(
      await this.mcpCallTool("ollama", "show_model", { model }, policyPath),
    );
  }

  leaseSecret(params: {
    secret_name: string;
    audience: string;
    ttl_seconds?: number;
    policy_path?: string;
  }) {
    return this.rpc<{
      lease: SecretLeaseDescriptor;
      value: string;
    }>("secret/lease", params);
  }

  revokeSecret(leaseId: string) {
    return this.rpc<{ revoked: boolean }>("secret/revoke", {
      lease_id: leaseId,
    });
  }

  listSecretLeases() {
    return this.rpc<unknown[]>("secret/list");
  }

  acpSpawn(params: {
    program: string;
    args?: string[];
    cwd?: string;
    environment?: Record<string, string>;
    timeout_seconds?: number;
    policy_path?: string;
  }) {
    return this.rpc<{ process_id: string }>("acp/spawn", params);
  }

  acpRequest(processId: string, method: string, params: unknown = null) {
    return this.rpc<{ result: unknown }>("acp/request", {
      process_id: processId,
      method,
      params,
    });
  }

  acpNotify(processId: string, method: string, params: unknown = null) {
    return this.rpc<{ ok: boolean }>("acp/notify", {
      process_id: processId,
      method,
      params,
    });
  }

  acpClose(processId: string) {
    return this.rpc<{ closed: boolean; persisted?: boolean; reason?: string }>(
      "acp/close",
      { process_id: processId },
    );
  }

  acpList() {
    return this.rpc<unknown[]>("acp/list");
  }

  mcpListTools(
    serverName: string,
    policyPath = "config/policies/development.yaml",
  ) {
    return this.rpc<
      Array<{
        name: string;
        description?: string;
        inputSchema: unknown;
        annotations?: unknown;
      }>
    >("mcp/list_tools", {
      server_name: serverName,
      policy_path: policyPath,
    });
  }

  mcpCallTool(
    serverName: string,
    toolName: string,
    args: unknown,
    policyPath?: string,
  ) {
    return this.rpc<{
      content: unknown[];
      isError: boolean;
      structuredContent?: unknown;
    }>("mcp/call_tool", {
      server_name: serverName,
      tool_name: toolName,
      arguments: args,
      policy_path: policyPath,
    });
  }

  mcpListResources(
    serverName: string,
    policyPath = "config/policies/development.yaml",
  ) {
    return this.rpc<unknown>("mcp/list_resources", {
      server_name: serverName,
      policy_path: policyPath,
    });
  }

  mcpReadResource(
    serverName: string,
    uri: string,
    policyPath = "config/policies/development.yaml",
  ) {
    return this.rpc<unknown>("mcp/read_resource", {
      server_name: serverName,
      uri,
      policy_path: policyPath,
    });
  }

  mcpListPrompts(
    serverName: string,
    policyPath = "config/policies/development.yaml",
  ) {
    return this.rpc<unknown>("mcp/list_prompts", {
      server_name: serverName,
      policy_path: policyPath,
    });
  }

  budgetReserve(runId: string, estimatedUsd: number) {
    return this.rpc<{
      reservation_id: string;
      estimated_usd: number;
    }>("budget/reserve", {
      run_id: runId,
      estimated_usd: estimatedUsd,
    });
  }

  budgetSettle(reservationId: string, actualUsd: number) {
    return this.rpc<{
      reservation_id: string;
      run_id: string;
      estimated_usd: number;
      actual_usd: number;
      created_at: string;
      settled_at: string;
    }>("budget/settle", {
      reservation_id: reservationId,
      actual_usd: actualUsd,
    });
  }

  budgetSnapshot(runId: string) {
    return this.rpc<BudgetSnapshot>("budget/snapshot", {
      run_id: runId,
    });
  }

  artifactStreamBegin(params: {
    media_type?: string;
    source?: string;
  } = {}) {
    return this.rpc<{ upload_id: string }>("artifact/stream/begin", params);
  }

  artifactStreamChunk(uploadId: string, base64: string) {
    return this.rpc<{ upload_id: string; bytes_written: number }>(
      "artifact/stream/chunk",
      { upload_id: uploadId, base64 },
    );
  }

  artifactStreamCommit(
    uploadId: string,
    metadata?: Record<string, unknown>,
  ) {
    return this.rpc<ArtifactDescriptor>("artifact/stream/commit", {
      upload_id: uploadId,
      metadata: metadata ?? {},
    });
  }

  artifactStreamAbort(uploadId: string) {
    return this.rpc<{ aborted: boolean }>("artifact/stream/abort", {
      upload_id: uploadId,
    });
  }

  listPlugins() {
    return this.rpc<PluginManifest[]>("plugins/list");
  }

  validatePluginCapability(pluginId: string, capability: string) {
    return this.rpc<PluginCapabilityDeclaration>(
      "plugins/capability/validate",
      { plugin_id: pluginId, capability },
    );
  }

  async health(): Promise<boolean> {
    try {
      const response = await fetch(this.baseUrl + "/health");
      return response.ok;
    } catch {
      return false;
    }
  }
}
