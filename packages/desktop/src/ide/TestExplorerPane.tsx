import { useEffect, useState } from "react";
import type { KnowledgeNode, OpenForgeClient } from "@openforge/sdk";

function commandFor(test: KnowledgeNode) {
  const lower = test.path.toLowerCase();
  if (lower.endsWith(".py")) {
    return "python -m pytest " + shellQuote(test.path);
  }
  if (lower.endsWith(".rs") || lower.includes("/tests/")) {
    return "cargo test";
  }
  if (
    lower.endsWith(".test.ts") ||
    lower.endsWith(".test.tsx") ||
    lower.endsWith(".spec.ts") ||
    lower.endsWith(".spec.tsx")
  ) {
    return "pnpm test -- " + shellQuote(test.path);
  }
  return "";
}

function shellQuote(value: string) {
  return "'" + value.replaceAll("'", "'\\''") + "'";
}

export function TestExplorerPane(props: {
  client: OpenForgeClient;
  repo: string;
  onOpenFile: (path: string) => void;
  onRunCommand: (command: string) => void;
}) {
  const { client, repo, onOpenFile, onRunCommand } = props;
  const [tests, setTests] = useState<KnowledgeNode[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!repo) return;
    void client
      .discoverTests(repo)
      .then((value) => {
        setTests(value);
        setError("");
      })
      .catch((failure: unknown) => {
        setError(failure instanceof Error ? failure.message : String(failure));
      });
  }, [client, repo]);

  return (
    <section className="tool-pane">
      <div className="pane-toolbar">
        <span>Tests</span>
        <span className="pane-status">{tests.length}</span>
      </div>
      {error && <div className="pane-error">{error}</div>}
      <div className="pane-scroll">
        {tests.map((test) => {
          const command = commandFor(test);
          return (
            <div className="test-row" key={test.id}>
              <button className="test-open" onClick={() => onOpenFile(test.path)}>
                <strong>{test.name}</strong>
                <small>
                  {test.path}:{test.start_line}
                </small>
              </button>
              {command && (
                <button
                  className="test-run"
                  onClick={() => onRunCommand(command)}
                  title={command}
                >
                  Run
                </button>
              )}
            </div>
          );
        })}
        {tests.length === 0 && !error && (
          <div className="pane-empty">No tests discovered in the semantic index.</div>
        )}
      </div>
    </section>
  );
}
