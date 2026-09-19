import { useCallback, useEffect, useState } from "react";
import type { GitStatusEntry, GitStatusResult, OpenForgeClient } from "@openforge/sdk";

function statusLabel(entry: GitStatusEntry) {
  const parts: string[] = [];
  if (entry.index !== " " && entry.index !== "?") parts.push("staged " + entry.index);
  if (entry.worktree !== " ") parts.push("working " + entry.worktree);
  if (entry.index === "?") parts.push("untracked");
  return parts.join(" · ") || "clean";
}

export function SourceControlPane(props: {
  client: OpenForgeClient;
  repo: string;
  onOpenFile: (path: string) => void;
}) {
  const { client, repo, onOpenFile } = props;
  const [status, setStatus] = useState<GitStatusResult>({ entries: [] });
  const [selected, setSelected] = useState<GitStatusEntry | null>(null);
  const [diff, setDiff] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    if (!repo) return;
    try {
      const next = await client.gitStatus(repo);
      setStatus(next);
      setError("");
      if (
        selected &&
        !next.entries.some((entry) => entry.path === selected.path)
      ) {
        setSelected(null);
        setDiff("");
      }
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }, [client, repo, selected]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function showDiff(entry: GitStatusEntry, staged: boolean) {
    setSelected(entry);
    try {
      const value = await client.gitDiff(repo, { path: entry.path, staged });
      setDiff(value.diff);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function stage(entry: GitStatusEntry) {
    await client.gitStage(repo, [entry.path]);
    await refresh();
  }

  async function unstage(entry: GitStatusEntry) {
    await client.gitUnstage(repo, [entry.path]);
    await refresh();
  }

  async function commit() {
    if (!message.trim()) return;
    try {
      await client.gitCommit(repo, message.trim());
      setMessage("");
      await refresh();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  return (
    <section className="scm-pane">
      <div className="pane-toolbar">
        <span>Source Control</span>
        <button onClick={() => void refresh()}>Refresh</button>
      </div>
      <div className="scm-branch">{status.branch ?? "detached / unknown branch"}</div>
      <div className="scm-commit">
        <textarea
          value={message}
          onChange={(event) => setMessage(event.target.value)}
          placeholder="Commit message"
          rows={2}
        />
        <button onClick={() => void commit()}>Commit staged</button>
      </div>
      {error && <div className="pane-error">{error}</div>}
      <div className="scm-files">
        {status.entries.map((entry) => {
          const staged = entry.index !== " " && entry.index !== "?";
          const working = entry.worktree !== " " || entry.index === "?";
          return (
            <div className="scm-entry" key={entry.path + ":" + entry.index + entry.worktree}>
              <button className="scm-path" onClick={() => onOpenFile(entry.path)}>
                <span>{entry.path}</span>
                <small>{statusLabel(entry)}</small>
              </button>
              <div className="scm-actions">
                {working && (
                  <>
                    <button onClick={() => void showDiff(entry, false)}>Diff</button>
                    <button onClick={() => void stage(entry)}>Stage</button>
                  </>
                )}
                {staged && (
                  <>
                    <button onClick={() => void showDiff(entry, true)}>Staged diff</button>
                    <button onClick={() => void unstage(entry)}>Unstage</button>
                  </>
                )}
              </div>
            </div>
          );
        })}
        {status.entries.length === 0 && (
          <div className="pane-empty">Working tree clean.</div>
        )}
      </div>
      {selected && (
        <pre className="diff-view" aria-label={"Diff for " + selected.path}>
          {diff || "No textual diff."}
        </pre>
      )}
    </section>
  );
}
