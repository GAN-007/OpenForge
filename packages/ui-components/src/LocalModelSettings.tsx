import { useCallback, useEffect, useMemo, useState } from "react";

interface ConfiguredModel {
  provider: string;
  model: string;
  family: string;
  context_tokens: number;
  supports_tools: boolean;
  supports_structured_output: boolean;
}

interface Preflight {
  provider: string;
  model: string;
  ready: boolean;
  endpoint_reachable?: boolean | null;
  installed?: boolean | null;
  loaded?: boolean | null;
  available_memory_mb?: number | null;
  required_memory_mb?: number | null;
  detail?: string | null;
}

interface InstalledModel {
  name: string;
  model?: string;
  size?: number;
  modified_at?: string;
}

interface LocalModelClient {
  listModels(): Promise<ConfiguredModel[]>;
  preflightModels(): Promise<Preflight[]>;
  ollamaListModels(): Promise<{ models: InstalledModel[] }>;
  ollamaPullModel(model: string): Promise<{
    model: string;
    status: string;
    completed: boolean;
  }>;
}

function gib(bytes?: number) {
  return bytes == null ? "unknown" : (bytes / 1024 ** 3).toFixed(1) + " GiB";
}

export function LocalModelSettings({ client }: { client: LocalModelClient }) {
  const [configured, setConfigured] = useState<ConfiguredModel[]>([]);
  const [installed, setInstalled] = useState<InstalledModel[]>([]);
  const [preflight, setPreflight] = useState<Preflight[]>([]);
  const [model, setModel] = useState("");
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const refresh = useCallback(async () => {
    try {
      const [models, checks, local] = await Promise.all([
        client.listModels(),
        client.preflightModels(),
        client.ollamaListModels(),
      ]);
      setConfigured(models.filter((candidate) => candidate.provider === "local"));
      setPreflight(checks.filter((candidate) => candidate.provider === "local"));
      setInstalled(local.models);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }, [client]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const availableMemory = useMemo(
    () => preflight.find((item) => item.available_memory_mb != null)?.available_memory_mb,
    [preflight],
  );

  async function pull(target: string) {
    const trimmed = target.trim();
    if (!trimmed) return;
    setBusy(trimmed);
    setNotice("");
    try {
      const result = await client.ollamaPullModel(trimmed);
      if (!result.completed) {
        throw new Error("Ollama pull did not report success: " + result.status);
      }
      setNotice(`Pulled ${result.model} successfully.`);
      setModel("");
      await refresh();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy("");
    }
  }

  const configuredNames = new Set(configured.map((item) => item.model));
  const extraInstalled = installed.filter(
    (item) => !configuredNames.has(item.name) && !configuredNames.has(item.model ?? ""),
  );

  return (
    <div className="local-models">
      <div className="local-models__summary">
        <strong>Native Ollama</strong>
        <span>
          {availableMemory == null
            ? "RAM headroom unavailable"
            : `${availableMemory.toLocaleString()} MiB RAM available`}
        </span>
        <button type="button" onClick={() => void refresh()} disabled={Boolean(busy)}>
          Refresh
        </button>
      </div>

      {error && <p role="alert">{error}</p>}
      {notice && <p role="status">{notice}</p>}

      <div className="local-models__grid">
        {configured.map((candidate) => {
          const check = preflight.find(
            (item) => item.provider === candidate.provider && item.model === candidate.model,
          );
          return (
            <article key={candidate.provider + "/" + candidate.model}>
              <div>
                <strong>{candidate.model}</strong>
                <small>
                  {candidate.family} · {candidate.context_tokens.toLocaleString()} ctx
                  {candidate.supports_tools ? " · tools" : ""}
                  {candidate.supports_structured_output ? " · JSON" : ""}
                </small>
              </div>
              <span className={check?.ready ? "local-models__ready" : "local-models__blocked"}>
                {check?.ready
                  ? check.loaded
                    ? "loaded"
                    : "ready"
                  : check?.installed === false
                    ? "not installed"
                    : "blocked"}
              </span>
              <small>{check?.detail ?? "Waiting for preflight."}</small>
              {check?.required_memory_mb != null && (
                <small>
                  RAM: {check.available_memory_mb?.toLocaleString() ?? "?"} / ~
                  {check.required_memory_mb.toLocaleString()} MiB
                </small>
              )}
              {check?.installed === false && (
                <button
                  type="button"
                  disabled={Boolean(busy)}
                  onClick={() => void pull(candidate.model)}
                >
                  {busy === candidate.model ? "Pulling…" : "Pull model"}
                </button>
              )}
            </article>
          );
        })}
      </div>

      <form
        className="local-models__pull"
        onSubmit={(event) => {
          event.preventDefault();
          void pull(model);
        }}
      >
        <label>
          Pull another Ollama model
          <input
            value={model}
            onChange={(event) => setModel(event.target.value)}
            placeholder="e.g. qwen2.5-coder:14b"
            pattern="[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}"
            disabled={Boolean(busy)}
          />
        </label>
        <button disabled={Boolean(busy) || !model.trim()} type="submit">
          {busy === model.trim() ? "Pulling…" : "Pull"}
        </button>
      </form>

      {extraInstalled.length > 0 && (
        <details>
          <summary>{extraInstalled.length} additional installed model(s)</summary>
          <ul>
            {extraInstalled.map((item) => (
              <li key={item.name}>
                <code>{item.name}</code> · {gib(item.size)}
              </li>
            ))}
          </ul>
        </details>
      )}

      <p>
        OpenForge routes requests automatically across configured models. Pulling a model installs it;
        routing still respects context, tool, structured-output, privacy, cost and latency constraints.
      </p>
    </div>
  );
}
