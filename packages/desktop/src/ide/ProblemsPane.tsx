export interface Problem {
  uri: string;
  path: string;
  line: number;
  column: number;
  endLine: number;
  endColumn: number;
  severity: number;
  message: string;
  source?: string;
  code?: string | number;
}

function label(severity: number) {
  if (severity === 1) return "Error";
  if (severity === 2) return "Warning";
  if (severity === 3) return "Info";
  return "Hint";
}

export function ProblemsPane(props: {
  problems: Problem[];
  onOpen: (problem: Problem) => void;
}) {
  const { problems, onOpen } = props;
  return (
    <section className="tool-pane">
      <div className="pane-toolbar">
        <span>Problems</span>
        <span className="pane-status">{problems.length}</span>
      </div>
      <div className="pane-scroll">
        {problems.length === 0 ? (
          <div className="pane-empty">No diagnostics reported by active language servers.</div>
        ) : (
          problems.map((problem, index) => (
            <button
              className={"problem-row severity-" + problem.severity}
              key={problem.uri + ":" + problem.line + ":" + problem.column + ":" + index}
              onClick={() => onOpen(problem)}
            >
              <span className="problem-severity">{label(problem.severity)}</span>
              <span className="problem-message">{problem.message}</span>
              <span className="problem-location">
                {problem.path}:{problem.line}:{problem.column}
              </span>
            </button>
          ))
        )}
      </div>
    </section>
  );
}
