import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  OpenForgeClient,
  listOllamaModels,
  preflightOllamaModel,
  pullOllamaModel,
  type EventEnvelope,
  type EventIntegrityReport,
  type ModelSpec,
  type OllamaInventory,
  type OllamaPreflight,
  type Run,
  type SearchHit,
  type TaskNode,
  type TelemetrySnapshot,
} from "@openforge/sdk";
import { EmptyState, GatewaySettings, LocalModelSettings, Panel, StatusPill } from "@openforge/ui";

function formatBytes(value: number | null | undefined): string {
  if (value == null) return "unavailable";
  const gib = value / (1024 ** 3);
  return gib >= 1 ? `${gib.toFixed(1)} GiB` : `${(value / (1024 ** 2)).toFixed(0)} MiB`;
}

export function App() {
  const [apiToken, setApiToken] = useState(
    () => window.sessionStorage.getItem("openforge.apiToken") ?? "",
  );
  const client = useMemo(
    () => new OpenForgeClient(import.meta.env.VITE_OPENFORGE_DAEMON_URL || "http://127.0.0.1:8765", apiToken || undefined),
    [apiToken],
  );
  const [runs, setRuns] = useState<Run[]>([]);
  const [objective, setObjective] = useState("");
  const [budget, setBudget] = useState("10");
  const [busy, setBusy] = useState(false);
  const [streamStatus, setStreamStatus] = useState("No run selected");
  const [online, setOnline] = useState(false);
  const [runId, setRunId] = useState("");
  const [run, setRun] = useState<Run | null>(null);
  const [tasks, setTasks] = useState<TaskNode[]>([]);
  const [events, setEvents] = useState<EventEnvelope[]>([]);
  const [cost, setCost] = useState(0);
  const [integrity, setIntegrity] = useState<EventIntegrityReport | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetrySnapshot | null>(null);
  const [repo, setRepo] = useState("");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [error, setError] = useState("");
  const [configuredModels, setConfiguredModels] = useState<ModelSpec[]>([]);
  const [ollamaInventory, setOllamaInventory] = useState<OllamaInventory | null>(null);
  const [ollamaPreflight, setOllamaPreflight] = useState<Record<string, OllamaPreflight>>({});
  const [ollamaInput, setOllamaInput] = useState("");
  const [ollamaBusy, setOllamaBusy] = useState(false);
  const [ollamaError, setOllamaError] = useState("");

  const refreshVersion = useRef(0);
  const refresh = useCallback(async () => {
    const version = ++refreshVersion.current;
    const healthy = await client.health();
    if (version !== refreshVersion.current) return;
    setOnline(healthy);
    try {
      const [audit, metrics, recentRuns] = await Promise.all([
        client.verifyEvents(),
        client.telemetry(),
        client.listRuns(),
      ]);
      if (version !== refreshVersion.current) return;
      setRuns(recentRuns);
      setIntegrity(audit);
      setTelemetry(metrics);

      if (!runId.trim()) {
        setError("");
        return;
      }

      const [currentRun, currentTasks, budget] =
        await Promise.all([
          client.getRun(runId.trim()),
          client.listTasks(runId.trim()),
          client.getBudget(runId.trim()),
        ]);
      if (version !== refreshVersion.current) return;
      setRun(currentRun);
      setTasks(currentTasks);
      setCost(budget.spent_usd);
      setError("");
    } catch (err) {
      if (version === refreshVersion.current) setError(err instanceof Error ? err.message : String(err));
    }
  }, [client, runId]);

  const refreshOllama = useCallback(async () => {
    setOllamaBusy(true);
    try {
      const catalog = await client.listModels();
      const local = catalog.filter((model) => model.provider === "local");
      setConfiguredModels(local);
      const inventory = await listOllamaModels(client);
      setOllamaInventory(inventory);
      const checks = await Promise.all(
        local.map(async (model) => {
          try {
            const check = await preflightOllamaModel(client, model.model, model.context_tokens);
            return [model.model, check] as const;
          } catch (err) {
            return [
              model.model,
              {
                ready: false,
                installed: false,
                model: model.model,
                context_tokens: model.context_tokens,
                available_memory_bytes: inventory.available_memory_bytes,
                estimated_required_memory_bytes: null,
                message: err instanceof Error ? err.message : String(err),
              } satisfies OllamaPreflight,
            ] as const;
          }
        }),
      );
      setOllamaPreflight(Object.fromEntries(checks));
      setOllamaError("");
    } catch (err) {
      setOllamaInventory(null);
      setOllamaPreflight({});
      setOllamaError(err instanceof Error ? err.message : String(err));
    } finally {
      setOllamaBusy(false);
    }
  }, [client]);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), 5000);
    return () => { window.clearInterval(id); ++refreshVersion.current; };
  }, [refresh]);

  useEffect(() => {
    void refreshOllama();
  }, [refreshOllama]);

  useEffect(() => {
    setRun(null);
    setTasks([]);
    setEvents([]);
    setCost(0);
    if (!runId.trim()) {
      setStreamStatus("No run selected");
      return;
    }
    const controller = new AbortController();
    let cursor = 0;
    let retryTimer: number | undefined;
    async function follow() {
      setStreamStatus("Connecting to audit stream…");
      try {
        for await (const event of client.streamEvents(runId.trim(), cursor, controller.signal)) {
          if (controller.signal.aborted) return;
          cursor = event.sequence;
          setStreamStatus("Live audit stream");
          setEvents((current) => [...current.filter((item) => item.event_id !== event.event_id), event].slice(-500));
        }
      } catch {
        if (controller.signal.aborted) return;
      }
      if (!controller.signal.aborted) {
        setStreamStatus("Reconnecting to audit stream…");
        retryTimer = window.setTimeout(() => void follow(), 3000);
      }
    }
    void follow();
    return () => { controller.abort(); window.clearTimeout(retryTimer); };
  }, [client, runId]);

  async function createRun() {
    if (!repo.trim() || !objective.trim() || !Number.isFinite(Number(budget)) || Number(budget) <= 0) {
      setError("Enter a repository, objective, and positive budget.");
      return;
    }
    setBusy(true);
    try {
      const created = await client.createRun({ repo: repo.trim(), objective: objective.trim(), budget_usd: Number(budget), autonomy: "suggest" });
      setRuns((current) => [created, ...current]);
      setRunId(created.id);
      setError("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally { setBusy(false); }
  }

  async function planRun() {
    setBusy(true);
    try {
      setTasks(await client.planRun(repo.trim(), runId.trim()));
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally { setBusy(false); }
  }

  async function searchRepository() {
    if (!repo.trim() || !query.trim()) return;
    try {
      const result = await client.search(repo.trim(), query.trim(), 30);
      setHits(result.hits);
      setError("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function pullModel(model: string) {
    if (!model.trim()) return;
    setOllamaBusy(true);
    try {
      await pullOllamaModel(client, model.trim());
      setOllamaInput("");
      await refreshOllama();
    } catch (err) {
      setOllamaError(err instanceof Error ? err.message : String(err));
      setOllamaBusy(false);
    }
  }

  function persistToken(value: string) {
    setApiToken(value);
    if (value) {
      window.sessionStorage.setItem("openforge.apiToken", value);
    } else {
      window.sessionStorage.removeItem("openforge.apiToken");
    }
  }

  const running = tasks.filter((task) => task.status === "running").length;
  const completed = tasks.filter((task) => task.status === "completed").length;
  const failed = tasks.filter((task) => task.status === "failed").length;

  return (
    <main className="shell">
      <header className="topbar">
        <div>
          <h1>OpenForge</h1>
          <p>AI Engineering Control Plane · Protocol v2</p>
        </div>
        <div className="connection">
          <span className={online ? "dot dot--ok" : "dot"} />
          {online ? "daemon online" : "daemon offline"}
        </div>
      </header>

      <div className="toolbar">
        <select aria-label="Recent runs" value={runs.some((item) => item.id === runId) ? runId : ""} onChange={(event) => setRunId(event.target.value)}>
          <option value="">Select a recent run</option>
          {runs.map((item) => <option key={item.id} value={item.id}>{item.objective.slice(0, 70)} · {item.status}</option>)}
        </select>
        <input
          aria-label="Run ID"
          value={runId}
          onChange={(event) => setRunId(event.target.value)}
          placeholder="Paste a run UUID"
        />
        <input
          aria-label="OpenForge daemon access token"
          type="password"
          value={apiToken}
          onChange={(event) => persistToken(event.target.value)}
          placeholder="Optional daemon access token (not the Sevi key)"
        />
        <button onClick={() => void refresh()}>Refresh</button>
        {run && (
          <>
            <StatusPill status={run.status} />
            <span className="cost">${cost.toFixed(4)}</span>
          </>
        )}
      </div>

      {error && <div className="error">{error}</div>}

      <Panel title="Model connection">
        <GatewaySettings client={client} />
      </Panel>

      <Panel title="Local Ollama models" className="ollama-models">
        <div className="ollama-summary">
          <div>
            <strong>{ollamaInventory ? `${ollamaInventory.models.length} installed` : "Ollama unavailable"}</strong>
            <span>Available RAM: {formatBytes(ollamaInventory?.available_memory_bytes)}</span>
          </div>
          <div className="ollama-actions">
            <input
              aria-label="Ollama model to pull"
              value={ollamaInput}
              onChange={(event) => setOllamaInput(event.target.value)}
              placeholder="model:tag"
              onKeyDown={(event) => {
                if (event.key === "Enter") void pullModel(ollamaInput);
              }}
            />
            <button disabled={ollamaBusy || !ollamaInput.trim()} onClick={() => void pullModel(ollamaInput)}>Pull</button>
            <button disabled={ollamaBusy} onClick={() => void refreshOllama()}>{ollamaBusy ? "Checking…" : "Refresh"}</button>
          </div>
        </div>
        {ollamaError && <div className="error">{ollamaError}</div>}
        {configuredModels.length ? (
          <div className="ollama-grid">
            {configuredModels.map((model) => {
              const installed = ollamaInventory?.models.find((item) => item.name === model.model || item.model === model.model);
              const check = ollamaPreflight[model.model];
              return (
                <article className="ollama-card" key={`${model.provider}:${model.model}`}>
                  <div className="ollama-card__head">
                    <strong>{model.model}</strong>
                    <span className={check?.ready ? "fit fit--ok" : "fit"}>{check?.ready ? "fits" : installed ? "check RAM" : "not installed"}</span>
                  </div>
                  <span>{model.family} · {(model.context_tokens / 1024).toFixed(0)}k context · {model.supports_tools ? "tools" : "no tools"}</span>
                  <span>Model size: {formatBytes(installed?.size)} · estimated: {formatBytes(check?.estimated_required_memory_bytes)}</span>
                  <small>{check?.message ?? "Run refresh to preflight this model."}</small>
                  {!installed && <button disabled={ollamaBusy} onClick={() => void pullModel(model.model)}>Pull {model.model}</button>}
                </article>
              );
            })}
          </div>
        ) : (
          <EmptyState>No local provider models are configured.</EmptyState>
        )}
      </Panel>

      <div className="grid">
        <Panel title="Create a run" className="objective">
          <form onSubmit={(event) => { event.preventDefault(); void createRun(); }}>
            <label>Repository path<input required value={repo} onChange={(event) => setRepo(event.target.value)} placeholder="/absolute/path/to/repository" /></label>
            <label>Objective<input required value={objective} onChange={(event) => setObjective(event.target.value)} placeholder="What should OpenForge work on?" /></label>
            <label>Budget (USD)<input required type="number" min="0.01" step="0.01" value={budget} onChange={(event) => setBudget(event.target.value)} /></label>
            <button disabled={busy} type="submit">{busy ? "Working…" : "Create run"}</button>
            <button disabled={busy || !run || !repo.trim() || run.status !== "planning" || tasks.length > 0} type="button" onClick={() => void planRun()}>Plan selected run</button>
            <p>Creates a suggestion-mode run. Planning uses your configured model and budget.</p>
          </form>
        </Panel>
        <Panel title="Objective" className="objective">
          {run ? (
            <>
              <h2>{run.objective}</h2>
              <dl>
                <dt>Base SHA</dt>
                <dd>{run.base_sha}</dd>
                <dt>Autonomy</dt>
                <dd>{run.autonomy}</dd>
                <dt>Budget</dt>
                <dd>${run.budget.hard_limit.toFixed(2)}</dd>
              </dl>
            </>
          ) : (
            <EmptyState>Enter a run ID to inspect an engineering run.</EmptyState>
          )}
        </Panel>

        <Panel title="Agents / Tasks" className="tasks">
          {tasks.length ? (
            tasks.map((task) => (
              <article className="task" key={task.id}>
                <div>
                  <strong>{task.role}</strong>
                  <span>{task.title}</span>
                  <small>
                    {task.requirements.capabilities.join(" · ") || "no capabilities"}
                    {" · "}
                    {task.requirements.resources.cpu_cores} CPU
                    {" · "}
                    {task.requirements.resources.memory_mb} MB
                  </small>
                </div>
                <StatusPill status={task.status} />
              </article>
            ))
          ) : (
            <EmptyState>No task graph loaded.</EmptyState>
          )}
        </Panel>

        <Panel title="Task Graph" className="graph">
          {tasks.length ? (
            <svg
              viewBox={`0 0 800 ${Math.max(220, tasks.length * 62)}`}
              role="img"
              aria-label="Task dependency graph"
            >
              {tasks.flatMap((task, index) =>
                task.dependencies.map((dependency) => {
                  const parent = tasks.findIndex(
                    (candidate) => candidate.id === dependency,
                  );
                  if (parent < 0) return null;
                  return (
                    <line
                      key={dependency + task.id}
                      x1="210"
                      y1={parent * 58 + 34}
                      x2="570"
                      y2={index * 58 + 34}
                      stroke="currentColor"
                      opacity=".25"
                    />
                  );
                }),
              )}
              {tasks.map((task, index) => (
                <g
                  key={task.id}
                  transform={`translate(${index % 2 ? 530 : 20} ${index * 58 + 10})`}
                >
                  <rect width="250" height="44" rx="10" />
                  <text x="12" y="18">
                    {task.role}
                  </text>
                  <text className="sub" x="12" y="34">
                    {task.title.slice(0, 32)}
                  </text>
                </g>
              ))}
            </svg>
          ) : (
            <EmptyState>The DAG appears when tasks are planned.</EmptyState>
          )}
        </Panel>

        <Panel title="Audit / History" className="events">
          <p role="status">{streamStatus} · showing the latest 500 events</p>
          <div className="integrity">
            <strong>
              {integrity?.valid ? "Audit chain valid" : "Audit chain unverified"}
            </strong>
            <span>{integrity?.verified_events ?? 0} events verified</span>
          </div>
          {integrity && !integrity.valid && integrity.violations.length > 0 && (
            <div className="error">
              {integrity.violations.slice(0, 3).join("; ")}
            </div>
          )}
          {events.length ? (
            <ol>
              {events
                .slice()
                .reverse()
                .map((event) => (
                  <li key={event.event_id}>
                    <time>{new Date(event.timestamp).toLocaleTimeString()}</time>
                    <strong>{event.actor.id}</strong>
                    <span>{event.event_type}</span>
                  </li>
                ))}
            </ol>
          ) : (
            <EmptyState>No run events loaded.</EmptyState>
          )}
        </Panel>

        <Panel title="Repository Intelligence" className="repository">
          <div className="search-row">
            <input
              value={repo}
              onChange={(event) => setRepo(event.target.value)}
              placeholder="/path/to/repository"
            />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search code, concepts, symbols"
              onKeyDown={(event) => {
                if (event.key === "Enter") void searchRepository();
              }}
            />
            <button onClick={() => void searchRepository()}>Search</button>
          </div>
          {hits.length ? (
            <ol className="search-results">
              {hits.map((hit) => (
                <li key={`${hit.path}:${hit.line}`}>
                  <strong>
                    {hit.path}:{hit.line}
                  </strong>
                  <span>{hit.snippet}</span>
                </li>
              ))}
            </ol>
          ) : (
            <EmptyState>Search the daemon-built BM25 repository index.</EmptyState>
          )}
        </Panel>

        <Panel title="Security, Cost & Runtime" className="metrics">
          <div className="metric">
            <span>Spent</span>
            <strong>${cost.toFixed(4)}</strong>
          </div>
          <div className="metric">
            <span>Running</span>
            <strong>{running}</strong>
          </div>
          <div className="metric">
            <span>Completed</span>
            <strong>{completed}</strong>
          </div>
          <div className="metric">
            <span>Failed</span>
            <strong>{failed}</strong>
          </div>
          <div className="metric">
            <span>RPC Calls</span>
            <strong>{telemetry?.counters["rpc.calls"] ?? 0}</strong>
          </div>
          <div className="metric">
            <span>RPC Errors</span>
            <strong>{telemetry?.counters["rpc.errors"] ?? 0}</strong>
          </div>
        </Panel>
      </div>
    </main>
  );
}
