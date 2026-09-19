import { useEffect, useMemo, useState } from "react";
import type {
  DapEvent,
  DebugAdapterConfig,
  OpenForgeClient,
} from "@openforge/sdk";

export function DebugPane(props: {
  client: OpenForgeClient;
  repo: string;
}) {
  const { client, repo } = props;
  const [adapters, setAdapters] = useState<DebugAdapterConfig[]>([]);
  const [adapter, setAdapter] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [launchJson, setLaunchJson] = useState("{}");
  const [threadId, setThreadId] = useState("");
  const [events, setEvents] = useState<DapEvent[]>([]);
  const [inspection, setInspection] = useState<unknown>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    void client.listDebugAdapters().then((value) => {
      setAdapters(value);
      if (value[0]) setAdapter(value[0].name);
    });
  }, [client]);

  useEffect(() => {
    if (!sessionId) return;
    const timer = window.setInterval(() => {
      void client
        .dapEvents(sessionId)
        .then((value) => {
          if (value.length) setEvents((current) => [...current, ...value].slice(-500));
        })
        .catch(() => {});
    }, 600);
    return () => window.clearInterval(timer);
  }, [client, sessionId]);

  const parsedThread = useMemo(() => {
    const value = Number(threadId);
    return Number.isSafeInteger(value) && value > 0 ? value : null;
  }, [threadId]);

  async function start() {
    if (!adapter || !repo) return;
    try {
      const result = await client.startDebugAdapter(adapter, repo);
      setSessionId(result.session_id);
      setInspection(result.capabilities);
      setEvents([]);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function launch() {
    if (!sessionId) return;
    try {
      const args = JSON.parse(launchJson) as unknown;
      await client.dapRequest(sessionId, "launch", args);
      await client.dapRequest(sessionId, "configurationDone", {});
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function command(name: string) {
    if (!sessionId) return;
    try {
      const args = parsedThread ? { threadId: parsedThread } : {};
      const result = await client.dapRequest(sessionId, name, args);
      setInspection(result);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  return (
    <section className="debug-pane">
      <div className="pane-toolbar">
        <span>Debugger</span>
        <span className="pane-status">{sessionId ? "session " + sessionId.slice(0, 8) : "idle"}</span>
      </div>
      <div className="debug-controls">
        <select value={adapter} onChange={(event) => setAdapter(event.target.value)}>
          {adapters.map((item) => (
            <option key={item.name} value={item.name}>
              {item.name} · {item.adapter_id}
            </option>
          ))}
        </select>
        <button onClick={() => void start()} disabled={!adapter}>Start adapter</button>
        <input
          value={threadId}
          onChange={(event) => setThreadId(event.target.value)}
          placeholder="Thread ID"
        />
        <button onClick={() => void command("threads")} disabled={!sessionId}>Threads</button>
        <button onClick={() => void command("continue")} disabled={!sessionId || !parsedThread}>Continue</button>
        <button onClick={() => void command("next")} disabled={!sessionId || !parsedThread}>Step over</button>
        <button onClick={() => void command("stepIn")} disabled={!sessionId || !parsedThread}>Step in</button>
        <button onClick={() => void command("stepOut")} disabled={!sessionId || !parsedThread}>Step out</button>
        <button onClick={() => void command("pause")} disabled={!sessionId || !parsedThread}>Pause</button>
      </div>
      <div className="debug-launch">
        <textarea
          value={launchJson}
          onChange={(event) => setLaunchJson(event.target.value)}
          spellCheck={false}
          aria-label="Debug launch arguments JSON"
        />
        <button onClick={() => void launch()} disabled={!sessionId}>Launch</button>
      </div>
      {error && <div className="pane-error">{error}</div>}
      <div className="debug-grid">
        <pre>{inspection ? JSON.stringify(inspection, null, 2) : "No debug inspection data."}</pre>
        <pre>{events.length ? JSON.stringify(events, null, 2) : "No DAP events yet."}</pre>
      </div>
    </section>
  );
}
