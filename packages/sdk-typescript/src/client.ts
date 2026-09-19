import type {
  AgentThread,
  ApprovalRequest,
  ArtifactDescriptor,
  CapabilitySet,
  DapEvent,
  DebugAdapterConfig,
  DebugPhase,
  DebugSession,
  DurableJob,
  EditPrediction,
  EditPredictionInput,
  EventEnvelope,
  EventIntegrityReport,
  GitStatusResult,
  JobCheckpoint,
  JobLease,
  KnowledgeGraph,
  KnowledgeNode,
  LanguageServerConfig,
  LoadedPlugin,
  PluginInvocation,
  Presence,
  QueuedInstruction,
  RepositoryIndex,
  RichMemoryHit,
  RichMemoryInput,
  RichMemoryRecord,
  Run,
  SearchHit,
  SymbolGraph,
  SymbolRecord,
  TaskNode,
  TeamIdentity,
  TeamRole,
  TeamWorkspace,
  TelemetrySnapshot,
  TerminalDescriptor,
  TerminalEvent,
  ThreadMessage,
  ThreadStatus,
  WorkerDescriptor,
  WorkspaceFile,
  WorkspaceRepository,
} from "./types.js";

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
    params: object = {},
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
  }) {
    return this.rpc<{ integration_branch: string }>(
      "run/execute",
      params,
    );
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

  providers() {
    return this.rpc<{ providers: string[] }>("model/providers");
  }

  agents() {
    return this.rpc<unknown[]>("agent/list");
  }

  readFile(repo: string, path: string) {
    return this.rpc<WorkspaceFile>("workspace/file-read", { repo, path });
  }

  writeFile(repo: string, path: string, content: string) {
    return this.rpc<{ path: string; bytes: number }>("workspace/file-write", {
      repo,
      path,
      content,
    });
  }

  deleteFile(repo: string, path: string) {
    return this.rpc<{ deleted: boolean; path: string }>(
      "workspace/file-delete",
      { repo, path },
    );
  }

  renameFile(repo: string, from: string, to: string) {
    return this.rpc<{ renamed: boolean; from: string; to: string }>(
      "workspace/file-rename",
      { repo, from, to },
    );
  }

  createDirectory(repo: string, path: string) {
    return this.rpc<{ created: boolean; path: string }>(
      "workspace/directory-create",
      { repo, path },
    );
  }

  gitStatus(repo: string) {
    return this.rpc<GitStatusResult>("git/status", { repo });
  }

  gitDiff(repo: string, options: { staged?: boolean; path?: string } = {}) {
    return this.rpc<{ diff: string }>("git/diff", {
      repo,
      ...options,
    });
  }

  gitStage(repo: string, paths: string[]) {
    return this.rpc<{ staged: string[] }>("git/stage", { repo, paths });
  }

  gitUnstage(repo: string, paths: string[]) {
    return this.rpc<{ unstaged: string[] }>("git/unstage", { repo, paths });
  }

  gitCommit(repo: string, message: string) {
    return this.rpc<{ output: string }>("git/commit", { repo, message });
  }

  gitLog(repo: string, limit = 50) {
    return this.rpc<{ log: string }>("git/log", { repo, limit });
  }

  gitBranches(repo: string) {
    return this.rpc<{ branches: string }>("git/branches", { repo });
  }

  discoverTests(repo: string) {
    return this.rpc<KnowledgeNode[]>("test/discover", { repo });
  }

  buildKnowledge(repo: string) {
    return this.rpc<KnowledgeGraph>("knowledge/build", { repo });
  }

  searchKnowledge(repo: string, query: string, limit = 25) {
    return this.rpc<{
      symbols: KnowledgeNode[];
      files_indexed: number;
      nodes: number;
      edges: number;
    }>("knowledge/search", { repo, query, limit });
  }

  knowledgeNeighbors(repo: string, nodeId: string, depth = 2) {
    return this.rpc<KnowledgeNode[]>("knowledge/neighbors", {
      repo,
      node_id: nodeId,
      depth,
    });
  }

  predictEdits(input: EditPredictionInput, signal?: AbortSignal) {
    return this.rpc<EditPrediction>("edit/predict", { ...input }, signal);
  }

  listLanguageServers() {
    return this.rpc<LanguageServerConfig[]>("lsp/list");
  }

  startLanguageServer(name: string, repo: string) {
    return this.rpc<{ session_id: string; capabilities: unknown }>(
      "lsp/start",
      { name, repo },
    );
  }

  lspRequest<T>(
    sessionId: string,
    requestMethod: string,
    requestParams: unknown,
  ) {
    return this.rpc<T>("lsp/request", {
      session_id: sessionId,
      request_method: requestMethod,
      request_params: requestParams,
    });
  }

  lspNotify(
    sessionId: string,
    notificationMethod: string,
    notificationParams: unknown,
  ) {
    return this.rpc<{ ok: boolean }>("lsp/notify", {
      session_id: sessionId,
      notification_method: notificationMethod,
      notification_params: notificationParams,
    });
  }

  lspNotifications(sessionId: string) {
    return this.rpc<unknown[]>("lsp/notifications", {
      session_id: sessionId,
    });
  }

  stopLanguageServer(sessionId: string) {
    return this.rpc<{ stopped: boolean }>("lsp/stop", {
      session_id: sessionId,
    });
  }

  listDebugAdapters() {
    return this.rpc<DebugAdapterConfig[]>("dap/list");
  }

  startDebugAdapter(name: string, repo: string) {
    return this.rpc<{ session_id: string; capabilities: unknown }>(
      "dap/start",
      { name, repo },
    );
  }

  dapRequest<T>(
    sessionId: string,
    command: string,
    args: unknown = {},
  ) {
    return this.rpc<T>("dap/request", {
      session_id: sessionId,
      command,
      arguments: args,
    });
  }

  dapEvents(sessionId: string) {
    return this.rpc<DapEvent[]>("dap/events", {
      session_id: sessionId,
    });
  }

  stopDebugAdapter(sessionId: string, terminateDebuggee = false) {
    return this.rpc<{ stopped: boolean }>("dap/stop", {
      session_id: sessionId,
      terminate_debuggee: terminateDebuggee,
    });
  }

  spawnTerminal(params: {
    repo: string;
    program: string;
    args?: string[];
    cwd?: string;
    environment?: Record<string, string>;
    rows?: number;
    cols?: number;
  }) {
    return this.rpc<TerminalDescriptor>("terminal/spawn", params);
  }

  resizeTerminal(terminalId: string, rows: number, cols: number) {
    return this.rpc<{ ok: boolean }>("terminal/resize", {
      terminal_id: terminalId,
      rows,
      cols,
    });
  }

  listTerminals() {
    return this.rpc<TerminalDescriptor[]>("terminal/list");
  }

  closeTerminal(terminalId: string) {
    return this.rpc<{ closed: boolean }>("terminal/close", {
      terminal_id: terminalId,
    });
  }

  terminalSocket(terminalId: string): WebSocket {
    return new WebSocket(
      this.websocketUrl("/v1/terminal/" + encodeURIComponent(terminalId) + "/ws"),
      this.websocketProtocols(),
    );
  }

  runEventsSocket(runId: string): WebSocket {
    return new WebSocket(
      this.websocketUrl("/v1/runs/" + encodeURIComponent(runId) + "/events/ws"),
      this.websocketProtocols(),
    );
  }

  registerWorker(params: {
    name: string;
    endpoint?: string;
    capabilities?: string[];
    labels?: string[];
  }) {
    return this.rpc<WorkerDescriptor>("worker/register", params);
  }

  getWorker(workerId: string) {
    return this.rpc<WorkerDescriptor | null>("worker/get", {
      worker_id: workerId,
    });
  }

  workerHeartbeat(workerId: string) {
    return this.rpc<{ ok: boolean }>("worker/heartbeat", {
      worker_id: workerId,
    });
  }

  submitJob(params: {
    run_id: string;
    task_id?: string;
    payload?: unknown;
    required_capabilities?: string[];
    max_attempts?: number;
  }) {
    return this.rpc<DurableJob>("worker/submit", params);
  }

  claimJob(workerId: string, leaseSeconds = 60) {
    return this.rpc<JobLease | null>("worker/claim", {
      worker_id: workerId,
      lease_seconds: leaseSeconds,
    });
  }

  heartbeatLease(jobId: string, leaseToken: string, leaseSeconds = 60) {
    return this.rpc<{ ok: boolean }>("worker/lease-heartbeat", {
      job_id: jobId,
      lease_token: leaseToken,
      lease_seconds: leaseSeconds,
    });
  }

  checkpointJob(
    jobId: string,
    leaseToken: string,
    sequence: number,
    state: unknown,
  ) {
    return this.rpc<JobCheckpoint>("worker/checkpoint", {
      job_id: jobId,
      lease_token: leaseToken,
      sequence,
      state,
    });
  }

  latestCheckpoint(jobId: string) {
    return this.rpc<JobCheckpoint | null>("worker/checkpoint-latest", {
      job_id: jobId,
    });
  }

  completeJob(jobId: string, leaseToken: string, result: unknown) {
    return this.rpc<{ ok: boolean }>("worker/complete", {
      job_id: jobId,
      lease_token: leaseToken,
      result,
    });
  }

  failJob(jobId: string, leaseToken: string, error: string) {
    return this.rpc<{ ok: boolean }>("worker/fail", {
      job_id: jobId,
      lease_token: leaseToken,
      error,
    });
  }

  cancelJob(jobId: string) {
    return this.rpc<{ cancelled: boolean }>("worker/cancel", {
      job_id: jobId,
    });
  }

  requeueExpiredJobs() {
    return this.rpc<{ count: number }>("worker/requeue-expired");
  }

  putRichMemory(input: RichMemoryInput) {
    return this.rpc<RichMemoryRecord>("memory/rich-put", { ...input });
  }

  searchRichMemory(params: {
    scope?: string;
    query?: string;
    query_embedding?: number[];
    repository_fingerprint?: string;
    limit?: number;
  }) {
    return this.rpc<RichMemoryHit[]>("memory/rich-search", params);
  }

  deleteRichMemory(scope: string, key: string) {
    return this.rpc<{ deleted: boolean }>("memory/rich-delete", {
      scope,
      key,
    });
  }

  purgeExpiredMemory() {
    return this.rpc<{ deleted: number }>("memory/purge-expired");
  }

  invalidateRepositoryMemory(repositoryFingerprint: string) {
    return this.rpc<{ updated: number }>("memory/invalidate-repository", {
      repository_fingerprint: repositoryFingerprint,
    });
  }

  upsertIdentity(params: {
    provider: string;
    subject: string;
    email?: string;
    display_name?: string;
  }) {
    return this.rpc<TeamIdentity>("team/identity-upsert", params);
  }

  createWorkspace(slug: string, name: string) {
    return this.rpc<TeamWorkspace>("team/workspace-create", { slug, name });
  }

  setMembership(workspaceId: string, identityId: string, role: TeamRole) {
    return this.rpc<{ ok: boolean }>("team/membership-set", {
      workspace_id: workspaceId,
      identity_id: identityId,
      role,
    });
  }

  issueTeamToken(params: {
    workspace_id: string;
    identity_id: string;
    label: string;
    expires_at?: string;
  }) {
    return this.rpc<{ token: string }>("team/token-issue", params);
  }

  revokeTeamToken(token: string) {
    return this.rpc<{ revoked: boolean }>("team/token-revoke", { token });
  }

  registerWorkspaceRepository(
    workspaceId: string,
    name: string,
    path: string,
  ) {
    return this.rpc<WorkspaceRepository>("team/repository-register", {
      workspace_id: workspaceId,
      name,
      path,
    });
  }

  listWorkspaceRepositories(workspaceId?: string) {
    const params: Record<string, unknown> = {};
    if (workspaceId) params.workspace_id = workspaceId;
    return this.rpc<WorkspaceRepository[]>("team/repository-list", params);
  }

  createThread(runId: string, title: string, taskId?: string) {
    return this.rpc<AgentThread>("thread/create", {
      run_id: runId,
      title,
      ...(taskId ? { task_id: taskId } : {}),
    });
  }

  setThreadStatus(threadId: string, status: ThreadStatus) {
    return this.rpc<{ updated: boolean }>("thread/status", {
      thread_id: threadId,
      status,
    });
  }

  appendThreadMessage(
    threadId: string,
    content: unknown,
    messageType = "message",
    authorKind = "user",
  ) {
    return this.rpc<ThreadMessage>("thread/message", {
      thread_id: threadId,
      author_kind: authorKind,
      message_type: messageType,
      content,
    });
  }

  threadMessages(threadId: string, afterSequence = 0, limit = 500) {
    return this.rpc<ThreadMessage[]>("thread/messages", {
      thread_id: threadId,
      after_sequence: afterSequence,
      limit,
    });
  }

  queueInstruction(threadId: string, content: string) {
    return this.rpc<QueuedInstruction>("thread/instruction-queue", {
      thread_id: threadId,
      content,
    });
  }

  consumeInstructions(threadId: string, maximum = 100) {
    return this.rpc<QueuedInstruction[]>("thread/instruction-consume", {
      thread_id: threadId,
      maximum,
    });
  }

  requestApproval(
    threadId: string,
    capability: string,
    subject: string,
    reason: string,
  ) {
    return this.rpc<ApprovalRequest>("approval/request", {
      thread_id: threadId,
      capability,
      subject,
      reason,
    });
  }

  decideApproval(
    approvalId: string,
    status: "approved_once" | "approved_always" | "denied",
  ) {
    return this.rpc<{ updated: boolean }>("approval/decide", {
      approval_id: approvalId,
      status,
    });
  }

  heartbeatPresence(params: {
    workspace_id?: string;
    surface: string;
    resource?: string;
  }) {
    return this.rpc<Presence>("presence/heartbeat", params);
  }

  listPresence(workspaceId?: string, withinSeconds = 60) {
    return this.rpc<Presence[]>("presence/list", {
      ...(workspaceId ? { workspace_id: workspaceId } : {}),
      within_seconds: withinSeconds,
    });
  }

  loadPlugin(root: string) {
    return this.rpc<LoadedPlugin>("plugin/load", { root });
  }

  listPlugins() {
    return this.rpc<LoadedPlugin[]>("plugin/list");
  }

  unloadPlugin(id: string) {
    return this.rpc<{ unloaded: boolean }>("plugin/unload", { id });
  }

  invokePlugin(id: string, invocation: PluginInvocation) {
    return this.rpc<unknown>("plugin/invoke", { id, invocation });
  }

  browserRequest<T>(
    browserMethod: string,
    browserParams: Record<string, unknown> = {},
  ) {
    return this.rpc<T>("browser/request", {
      browser_method: browserMethod,
      browser_params: browserParams,
    });
  }

  browserNavigate(url: string, timeoutMs = 30_000) {
    return this.browserRequest<{ url: string; title: string; page_id?: string }>(
      "navigate",
      { url, timeout_ms: timeoutMs },
    );
  }

  browserScreenshot(fullPage = true) {
    return this.browserRequest<{ base64: string; url: string; page_id?: string }>(
      "screenshot",
      { full_page: fullPage },
    );
  }

  browserNetworkEntries(limit = 1000) {
    return this.browserRequest<{ entries: unknown[] }>("network/entries", {
      limit,
    });
  }

  browserConsole(limit = 1000) {
    return this.browserRequest<{ entries: string[] }>("console", { limit });
  }

  browserAccessibilitySnapshot(selector = "body") {
    return this.browserRequest<{ snapshot: string }>(
      "accessibility/snapshot",
      { selector },
    );
  }

  closeBrowser() {
    return this.rpc<{ closed: boolean }>("browser/close");
  }

  databaseIntrospect(repo: string, connection: unknown) {
    return this.rpc<unknown>("database/introspect", { repo, connection });
  }

  dockerInventory(repo: string) {
    return this.rpc<unknown>("devops/docker-inventory", { repo });
  }

  dockerInspect(repo: string, object: string) {
    return this.rpc<unknown>("devops/docker-inspect", { repo, object });
  }

  kubernetesInventory(repo: string, namespace?: string) {
    return this.rpc<unknown>("devops/kubernetes-inventory", {
      repo,
      ...(namespace ? { namespace } : {}),
    });
  }

  terraformPlan(repo: string, variableFiles: string[] = []) {
    return this.rpc<unknown>("devops/terraform-plan", {
      repo,
      variable_files: variableFiles,
    });
  }

  createDebugSession(runId: string, issue: string, taskId?: string) {
    return this.rpc<DebugSession>("debug/create", {
      run_id: runId,
      issue,
      ...(taskId ? { task_id: taskId } : {}),
    });
  }

  getDebugSession(debugId: string) {
    return this.rpc<DebugSession>("debug/get", { debug_id: debugId });
  }

  advanceDebugSession(debugId: string, phase: DebugPhase) {
    return this.rpc<DebugSession>("debug/advance", {
      debug_id: debugId,
      phase,
    });
  }

  addDebugHypothesis(
    debugId: string,
    statement: string,
    confidence: number,
  ) {
    return this.rpc<{ hypothesis_id: string }>("debug/hypothesis-add", {
      debug_id: debugId,
      statement,
      confidence,
    });
  }

  addDebugObservation(
    debugId: string,
    kind: string,
    source: string,
    data: unknown,
  ) {
    return this.rpc<{ observation_id: string }>("debug/observation-add", {
      debug_id: debugId,
      kind,
      source,
      data,
    });
  }

  attachDebugEvidence(
    debugId: string,
    hypothesisId: string,
    observationId: string,
    supports: boolean,
  ) {
    return this.rpc<DebugSession>("debug/evidence-attach", {
      debug_id: debugId,
      hypothesis_id: hypothesisId,
      observation_id: observationId,
      supports,
    });
  }

  rankDebugHypotheses(debugId: string) {
    return this.rpc<Array<[string, number]>>("debug/hypotheses-rank", {
      debug_id: debugId,
    });
  }

  selectDebugHypothesis(debugId: string, hypothesisId: string) {
    return this.rpc<DebugSession>("debug/hypothesis-select", {
      debug_id: debugId,
      hypothesis_id: hypothesisId,
    });
  }

  addDebugInstrumentation(
    debugId: string,
    description: string,
    file: string,
    reversiblePatch: string,
  ) {
    return this.rpc<{ instrumentation_id: string }>(
      "debug/instrumentation-add",
      {
        debug_id: debugId,
        description,
        file,
        reversible_patch: reversiblePatch,
      },
    );
  }

  setDebugFixSummary(debugId: string, summary: string) {
    return this.rpc<DebugSession>("debug/fix-summary", {
      debug_id: debugId,
      summary,
    });
  }

  setDebugVerificationSummary(debugId: string, summary: string) {
    return this.rpc<{ session: DebugSession; can_complete: boolean }>(
      "debug/verification-summary",
      { debug_id: debugId, summary },
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

  private websocketUrl(path: string): string {
    const url = new URL(this.baseUrl);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    url.pathname = path;
    url.search = "";
    url.hash = "";
    return url.toString();
  }

  private websocketProtocols(): string[] {
    const protocols = ["openforge-v1"];
    if (this.apiToken) {
      protocols.push("openforge-token." + base64Url(this.apiToken));
    }
    return protocols;
  }
}

function base64Url(value: string): string {
  const bytes = new TextEncoder().encode(value);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/g, "");
}
