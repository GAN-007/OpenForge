import { useEffect, useRef, useState } from "react";
import type {
  AutonomyLevel,
  EventEnvelope,
  OpenForgeClient,
  Run,
  TaskNode,
} from "@openforge/sdk";

export function AgentPane(props: {
  client: OpenForgeClient;
  repo: string;
  onRunChange: (runId: string) => void;
}) {
  const { client, repo, onRunChange } = props;
  const [objective, setObjective] = useState("");
  const [autonomy, setAutonomy] = useState<AutonomyLevel>("edit");
  const [budget, setBudget] = useState(5);
  const [run, setRun] = useState<Run | null>(null);
  const [tasks, setTasks] = useState<TaskNode[]>([]);
  const [events, setEvents] = useState<EventEnvelope[]>([]);
  const [executing, setExecuting] = useState(false);
  const [error, setError] = useState("");
  const socketRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    return () => socketRef.current?.close();
  }, []);

  async function refresh(runId = run?.id) {
    if (!runId) return;
    const [nextRun, nextTasks] = await Promise.all([
      client.getRun(runId),
      client.listTasks(runId),
    ]);
    if (nextRun) setRun(nextRun);
    setTasks(nextTasks);
  }

  function connectEvents(runId: string) {
    socketRef.current?.close();
    const socket = client.runEventsSocket(runId);
    socketRef.current = socket;
    socket.onmessage = (message) => {
      try {
        const event = JSON.parse(String(message.data)) as EventEnvelope;
        setEvents((current) => [...current, event].slice(-1000));
        void refresh(runId);
      } catch {
        // Event stream is deliberately fail-soft in the UI.
      }
    };
  }

  async function createAndPlan() {
    if (!repo || !objective.trim()) return;
    try {
      const created = await client.createRun({
        repo,
        objective: objective.trim(),
        autonomy,
        budget_usd: budget,
      });
      setRun(created);
      onRunChange(created.id);
      connectEvents(created.id);
      const planned = await client.planRun(repo, created.id);
      setTasks(planned);
      await refresh(created.id);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function execute() {
    if (!run) return;
    setExecuting(true);
    setError("");
    try {
      await client.executeRun({
        repo,
        run_id: run.id,
        docker: run.autonomy === "autonomous",
      });
      await refresh(run.id);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setExecuting(false);
    }
  }

  return (
    <section className="agent-pane">
      <div className="pane-toolbar">
        <span>Agents</span>
        <span className="pane-status">
          {run ? run.status + " · " + run.id.slice(0, 8) : "no run"}
        </span>
      </div>
      <div className="agent-create">
        <textarea
          value={objective}
          onChange={(event) => setObjective(event.target.value)}
          placeholder="Engineering objective"
          rows={3}
        />
        <select value={autonomy} onChange={(event) => setAutonomy(event.target.value as AutonomyLevel)}>
          <option value="observe">Observe</option>
          <option value="suggest">Suggest</option>
          <option value="edit">Edit</option>
          <option value="execute">Execute</option>
          <option value="autonomous">Autonomous</option>
        </select>
        <input
          type="number"
          min={0.1}
          step={0.5}
          value={budget}
          onChange={(event) => setBudget(Number(event.target.value))}
          aria-label="Run budget USD"
        />
        <button onClick={() => void createAndPlan()}>Create & plan</button>
        <button onClick={() => void execute()} disabled={!run || executing}>
          {executing ? "Executing…" : "Execute"}
        </button>
      </div>
      {error && <div className="pane-error">{error}</div>}
      <div className="agent-grid">
        <div className="task-list">
          {tasks.map((task) => (
            <div className={"task-card task-" + task.status} key={task.id}>
              <div>
                <strong>{task.title}</strong>
                <small>{task.role}</small>
              </div>
              <span>{task.status}</span>
              <small>
                {"attempts " + task.attempts + "/" + task.max_attempts + " · $" + task.budget.max_usd.toFixed(2)}
              </small>
            </div>
          ))}
          {tasks.length === 0 && (
            <div className="pane-empty">Create a run to see its task DAG.</div>
          )}
        </div>
        <div className="event-stream">
          {events.slice(-100).map((event) => (
            <div className="event-row" key={event.event_id}>
              <span>{event.sequence}</span>
              <strong>{event.event_type}</strong>
              <small>{event.actor.id}</small>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
