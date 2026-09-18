import type {
  CapabilitySet,
  EventEnvelope,
  Run,
  TaskNode,
} from "./types.js";

interface RpcResponse<T> {
  jsonrpc: "2.0";
  id: number;
  result?: T;
  error?: { code: number; message: string; data?: unknown };
}

export class OpenForgeClient {
  private nextId = 1;

  constructor(readonly baseUrl = "http://127.0.0.1:8765") {}

  async rpc<T>(
    method: string,
    params: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<T> {
    const id = this.nextId++;
    const response = await fetch(this.baseUrl + "/v1/rpc", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
      signal,
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

  listEvents(runId: string, afterSequence = 0) {
    return this.rpc<EventEnvelope[]>("event/list", {
      run_id: runId,
      after_sequence: afterSequence,
    });
  }

  getBudget(runId: string) {
    return this.rpc<{ run_id: string; spent_usd: number }>(
      "budget/get",
      { run_id: runId },
    );
  }

  searchMemory(
    query: string,
    scope?: string,
    limit = 100,
  ) {
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
