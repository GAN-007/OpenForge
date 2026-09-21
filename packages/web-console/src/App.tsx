import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import {
  OpenForgeClient,
  type EventEnvelope,
  type EventIntegrityReport,
  type ModelSettingsState,
  type Run,
  type SearchHit,
  type TaskNode,
  type TelemetrySnapshot,
} from "@openforge/sdk";
import { EmptyState, Panel, StatusPill } from "@openforge/ui";

export function App() {
  const [apiToken, setApiToken] = useState(
    () => window.sessionStorage.getItem("openforge.apiToken") ?? "",
  );
  const client = useMemo(
    () => new OpenForgeClient("http://127.0.0.1:8765", apiToken || undefined),
    [apiToken],
  );
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
  const [modelSettings, setModelSettings] = useState<ModelSettingsState | null>(null);
  const [modelKind, setModelKind] = useState<
    "openai-compatible" | "anthropic" | "gemini" | "bedrock-aws-cli"
  >("openai-compatible");
  const [modelProviderName, setModelProviderName] = useState("shared");
  const [modelBaseUrl, setModelBaseUrl] = useState("");
  const [modelId, setModelId] = useState("");
  const [modelFamily, setModelFamily] = useState("");
  const [modelApiKey, setModelApiKey] = useState("");

  const refresh = useCallback(async () => {
    setOnline(await client.health());
    try {
      const [audit, metrics] = await Promise.all([
        client.verifyEvents(),
        client.telemetry(),
      ]);
      setIntegrity(audit);
      setTelemetry(metrics);

      if (!runId.trim()) {
        setError("");
        return;
      }

      const [currentRun, currentTasks, currentEvents, budget] =
        await Promise.all([
          client.getRun(runId.trim()),
          client.listTasks(runId.trim()),
          client.listEvents(runId.trim()),
          client.getBudget(runId.trim()),
        ]);
      setRun(currentRun);
      setTasks(currentTasks);
      setEvents(currentEvents);
      setCost(budget.spent_usd);
      setError("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [client, runId]);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(id);
  }, [refresh]);

  useEffect(() => {
    void client
      .modelSettings()
      .then((state) => {
        setModelSettings(state);
        const settings = state.settings;
        if (settings) {
          setModelKind(settings.kind);
          setModelProviderName(settings.provider_name);
          setModelBaseUrl(settings.base_url);
          setModelId(settings.model);
          setModelFamily(settings.family);
        }
      })
      .catch((failure: unknown) => {
        setError(failure instanceof Error ? failure.message : String(failure));
      });
  }, [client]);

  async function saveModelSettings() {
    if (!modelId.trim()) {
      setError("Model ID is required.");
      return;
    }
    try {
      const state = await client.setModelSettings({
        provider_name: modelProviderName.trim() || "shared",
        kind: modelKind,
        base_url: modelBaseUrl.trim(),
        model: modelId.trim(),
        family: modelFamily.trim() || modelId.trim(),
        api_key: modelApiKey.trim() || undefined,
        retain_existing_api_key: true,
      });
      setModelSettings(state);
      setModelApiKey("");
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function clearModelSettings() {
    try {
      const state = await client.clearModelSettings();
      setModelSettings(state);
      setModelApiKey("");
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
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
        <input
          aria-label="Run ID"
          value={runId}
          onChange={(event) => setRunId(event.target.value)}
          placeholder="Paste a run UUID"
        />
        <input
          aria-label="API token"
          type="password"
          value={apiToken}
          onChange={(event) => persistToken(event.target.value)}
          placeholder="Optional API token (session only)"
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

      <div className="grid">
        <Panel title="Shared Model Runtime" className="objective">
          <p>
            Configure the daemon once. Browser, desktop, IDE and terminal clients
            use this same active provider and model.
          </p>
          <div className="search-row">
            <select
              aria-label="Provider kind"
              value={modelKind}
              onChange={(event) =>
                setModelKind(
                  event.target.value as
                    | "openai-compatible"
                    | "anthropic"
                    | "gemini"
                    | "bedrock-aws-cli",
                )
              }
            >
              <option value="openai-compatible">OpenAI-compatible gateway</option>
              <option value="anthropic">Anthropic</option>
              <option value="gemini">Gemini</option>
              <option value="bedrock-aws-cli">AWS Bedrock CLI</option>
            </select>
            <input
              value={modelProviderName}
              onChange={(event) => setModelProviderName(event.target.value)}
              placeholder="Provider name, e.g. sevi"
            />
            <input
              value={modelBaseUrl}
              onChange={(event) => setModelBaseUrl(event.target.value)}
              placeholder="Gateway base URL, e.g. https://gateway.example/v1"
            />
            <input
              value={modelId}
              onChange={(event) => setModelId(event.target.value)}
              placeholder="Model ID / alias"
            />
            <input
              value={modelFamily}
              onChange={(event) => setModelFamily(event.target.value)}
              placeholder="Model family (optional)"
            />
            <input
              type="password"
              value={modelApiKey}
              onChange={(event) => setModelApiKey(event.target.value)}
              placeholder={
                modelSettings?.settings?.api_key_configured
                  ? "API key already stored · enter only to replace"
                  : "API key / gateway token"
              }
            />
            <button onClick={() => void saveModelSettings()}>Use this model</button>
            <button onClick={() => void clearModelSettings()}>Use config default</button>
          </div>
          <small>
            Active:{" "}
            {modelSettings?.models?.length
              ? modelSettings.models
                  .map((model) => `${model.provider}/${model.model}`)
                  .join(", ")
              : "loading"}
            {modelSettings?.settings?.api_key_configured ? " · key stored" : ""}
          </small>
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
