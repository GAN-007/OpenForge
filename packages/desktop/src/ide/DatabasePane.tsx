import { useState } from "react";
import type { OpenForgeClient } from "@openforge/sdk";

type DatabaseKind =
  | "postgres"
  | "my_sql"
  | "sqlite"
  | "redis"
  | "mongo"
  | "click_house";

export function DatabasePane(props: {
  client: OpenForgeClient;
  repo: string;
}) {
  const { client, repo } = props;
  const [kind, setKind] = useState<DatabaseKind>("postgres");
  const [database, setDatabase] = useState("");
  const [environmentJson, setEnvironmentJson] = useState("{}");
  const [result, setResult] = useState<unknown>(null);
  const [error, setError] = useState("");

  async function inspect() {
    if (!database.trim()) return;
    try {
      const environment = JSON.parse(environmentJson) as Record<string, string>;
      const value = await client.databaseIntrospect(repo, {
        kind,
        database: database.trim(),
        environment,
      });
      setResult(value);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  return (
    <section className="database-pane">
      <div className="pane-toolbar">
        <span>Database</span>
        <span className="pane-status">read-only introspection</span>
      </div>
      <div className="database-controls">
        <select value={kind} onChange={(event) => setKind(event.target.value as DatabaseKind)}>
          <option value="postgres">PostgreSQL</option>
          <option value="my_sql">MySQL</option>
          <option value="sqlite">SQLite</option>
          <option value="redis">Redis</option>
          <option value="mongo">MongoDB</option>
          <option value="click_house">ClickHouse</option>
        </select>
        <input
          value={database}
          onChange={(event) => setDatabase(event.target.value)}
          placeholder="DSN, database path, Redis/Mongo URL, or ClickHouse host"
        />
        <button onClick={() => void inspect()}>Introspect</button>
      </div>
      <label className="database-env">
        Environment variables
        <textarea
          value={environmentJson}
          onChange={(event) => setEnvironmentJson(event.target.value)}
          spellCheck={false}
        />
      </label>
      {error && <div className="pane-error">{error}</div>}
      <pre className="database-result">
        {result ? JSON.stringify(result, null, 2) : "No schema inspection has been run."}
      </pre>
    </section>
  );
}
