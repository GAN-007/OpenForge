import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  OpenForgeClient,
  type CapabilitySet,
  type EventIntegrityReport,
  type RepositoryIndex,
  type SearchHit,
} from "@openforge/sdk";
import { EmptyState, GatewaySettings, Panel } from "@openforge/ui";

export function DesktopApp() {
  const [apiToken, setApiToken] = useState(
    () => window.sessionStorage.getItem("openforge.apiToken") ?? "",
  );
  const client = useMemo(
    () => new OpenForgeClient(import.meta.env.VITE_OPENFORGE_DAEMON_URL || "http://127.0.0.1:8765", apiToken || undefined),
    [apiToken],
  );
  const [caps, setCaps] = useState<CapabilitySet | null>(null);
  const [daemon, setDaemon] = useState("checking");
  const [repo, setRepo] = useState("");
  const [repository, setRepository] = useState<RepositoryIndex | null>(null);
  const [integrity, setIntegrity] = useState<EventIntegrityReport | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [error, setError] = useState("");
  const [system, setSystem] = useState<{
    platform: string;
    arch: string;
  } | null>(null);

  useEffect(() => {
    void client
      .initialize()
      .then(async (value) => {
        setCaps(value);
        setIntegrity(await client.verifyEvents());
        setDaemon("online");
        setError("");
      })
      .catch((failure: unknown) => {
        setDaemon("offline");
        setError(failure instanceof Error ? failure.message : String(failure));
      });
    void invoke<{ platform: string; arch: string }>("system_info").then(setSystem).catch(() => setSystem(null));
  }, [client]);

  function persistToken(value: string) {
    setApiToken(value);
    if (value) {
      window.sessionStorage.setItem("openforge.apiToken", value);
    } else {
      window.sessionStorage.removeItem("openforge.apiToken");
    }
  }

  async function inspectRepository() {
    if (!repo.trim()) return;
    try {
      setRepository(await client.repositoryIndex(repo.trim()));
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function searchRepository() {
    if (!repo.trim() || !query.trim()) return;
    try {
      const result = await client.search(repo.trim(), query.trim(), 20);
      setHits(result.hits);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  return (
    <main className="desktop">
      <header>
        <div>
          <h1>OpenForge</h1>
          <p>Engineering OS · Protocol v2</p>
        </div>
        <div className={`daemon daemon--${daemon}`}>{daemon}</div>
      </header>

      <nav>
        <button>Workspace</button>
        <button>Agents</button>
        <button>Graph</button>
        <button>Diff</button>
        <button>Browser</button>
        <button>Database</button>
        <button>Logs</button>
        <button>Cost</button>
        <button>Context</button>
        <button>History</button>
      </nav>

      <section className="workspace">
        <Panel title="Model connection" className="wide">
          <GatewaySettings client={client} />
        </Panel>
        <Panel title="Repository">
          <input
            value={repo}
            onChange={(event) => setRepo(event.target.value)}
            placeholder="/path/to/repository"
          />
          <div className="desktop-actions">
            <button onClick={() => void inspectRepository()}>Index</button>
          </div>
          <p>
            Repository operations are executed by the OpenForge daemon, policy
            broker, Git worktree manager, and sandbox boundary.
          </p>
        </Panel>

        <Panel title="Control Plane">
          <dl>
            <dt>Protocol</dt>
            <dd>{caps?.protocol_version ?? "unavailable"}</dd>
            <dt>Daemon</dt>
            <dd>{caps?.server_version ?? "unavailable"}</dd>
            <dt>Host</dt>
            <dd>{system ? `${system.platform}/${system.arch}` : "loading"}</dd>
            <dt>Audit</dt>
            <dd>
              {integrity
                ? integrity.valid
                  ? `valid · ${integrity.verified_events} events`
                  : `FAILED · ${integrity.violations.length} violations`
                : "unverified"}
            </dd>
          </dl>
          <input
            type="password"
            value={apiToken}
            onChange={(event) => persistToken(event.target.value)}
            aria-label="OpenForge daemon access token"
            placeholder="Optional daemon access token (not the Sevi key)"
          />
        </Panel>

        <Panel title="Capabilities" className="wide">
          <div className="capabilities">
            {Object.entries(caps?.capabilities ?? {}).map(([key, enabled]) => (
              <span key={key} className={enabled ? "enabled" : ""}>
                {key}
              </span>
            ))}
          </div>
        </Panel>

        <Panel title="Repository Index">
          {repository ? (
            <dl>
              <dt>Fingerprint</dt>
              <dd>{repository.fingerprint.slice(0, 20)}…</dd>
              <dt>Files</dt>
              <dd>{repository.files.length}</dd>
              <dt>Lines</dt>
              <dd>{repository.total_lines.toLocaleString()}</dd>
              <dt>Bytes</dt>
              <dd>{repository.total_bytes.toLocaleString()}</dd>
            </dl>
          ) : (
            <EmptyState>Index a repository to inspect its current snapshot.</EmptyState>
          )}
        </Panel>

        <Panel title="Repository Intelligence">
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void searchRepository();
            }}
            placeholder="Search code and concepts"
          />
          <div className="desktop-actions">
            <button onClick={() => void searchRepository()}>Search</button>
          </div>
          {hits.length > 0 && (
            <ol className="desktop-results">
              {hits.map((hit) => (
                <li key={`${hit.path}:${hit.line}`}>
                  <strong>
                    {hit.path}:{hit.line}
                  </strong>
                  <span>{hit.snippet}</span>
                </li>
              ))}
            </ol>
          )}
        </Panel>

        {error && (
          <Panel title="Control Plane Error" className="wide">
            <p className="desktop-error">{error}</p>
          </Panel>
        )}
      </section>
    </main>
  );
}
