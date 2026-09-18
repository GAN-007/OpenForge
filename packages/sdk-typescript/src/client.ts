import type {
  ArtifactDescriptor,
  CapabilitySet,
  EventEnvelope,
  EventIntegrityReport,
  RepositoryIndex,
  Run,
  SearchHit,
  SymbolGraph,
  SymbolRecord,
  TaskNode,
  TelemetrySnapshot,
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

  async health(): Promise<boolean> {
    try {
      const response = await fetch(this.baseUrl + "/health");
      return response.ok;
    } catch {
      return false;
    }
  }
}
