import { useEffect, useMemo, useRef, useState } from "react";
import Editor, { type Monaco, type OnMount } from "@monaco-editor/react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  OpenForgeClient,
  type CapabilitySet,
  type DiagnosticContext,
  type EventIntegrityReport,
  type FileRecord,
  type LanguageServerConfig,
  type RecentEdit,
  type RepositoryIndex,
  type SearchHit,
} from "@openforge/sdk";
import type { editor as MonacoEditor } from "monaco-editor";
import { AgentPane } from "./ide/AgentPane";
import { BrowserPane } from "./ide/BrowserPane";
import { DatabasePane } from "./ide/DatabasePane";
import { DebugPane } from "./ide/DebugPane";
import { registerEditPredictionProvider } from "./ide/EditPredictionBridge";
import { FileTree } from "./ide/FileTree";
import { registerLspProviders } from "./ide/LspBridge";
import { basename, fileUri, languageForPath, lspLanguageId } from "./ide/language";
import { ProblemsPane, type Problem } from "./ide/ProblemsPane";
import { SourceControlPane } from "./ide/SourceControlPane";
import { TerminalPane } from "./ide/TerminalPane";
import { TestExplorerPane } from "./ide/TestExplorerPane";

type BufferState = {
  path: string;
  content: string;
  savedContent: string;
  language: string;
  version: number;
};

type SidebarView = "explorer" | "search" | "source-control" | "agents";
type ToolView =
  | "terminal"
  | "problems"
  | "tests"
  | "debug"
  | "browser"
  | "database"
  | "output";

type LspSession = {
  id: string;
  config: LanguageServerConfig;
  capabilities: unknown;
};

type SystemInfo = {
  platform: string;
  arch: string;
};

function normalizePath(path: string) {
  return path.replaceAll("\\", "/");
}

function relativeFromUri(uri: string, repo: string) {
  try {
    const url = new URL(uri);
    let path = decodeURI(url.pathname);
    if (/^\/[A-Za-z]:\//.test(path)) path = path.slice(1);
    const root = normalizePath(repo).replace(/\/$/, "");
    path = normalizePath(path);
    if (path.startsWith(root + "/")) return path.slice(root.length + 1);
    return path;
  } catch {
    return uri;
  }
}

function toDiagnosticContext(problem: Problem): DiagnosticContext {
  return {
    file_path: problem.path,
    line: Math.max(0, problem.line - 1),
    column: Math.max(0, problem.column - 1),
    severity:
      problem.severity === 1
        ? "error"
        : problem.severity === 2
          ? "warning"
          : problem.severity === 3
            ? "info"
            : "hint",
    message: problem.message,
  };
}

function recentEdit(
  path: string,
  before: string,
  after: string,
  line: number,
  column: number,
): RecentEdit | null {
  if (before === after) return null;

  let start = 0;
  const maximum = Math.min(before.length, after.length);
  while (start < maximum && before[start] === after[start]) start += 1;

  let suffix = 0;
  while (
    suffix < before.length - start &&
    suffix < after.length - start &&
    before[before.length - suffix - 1] === after[after.length - suffix - 1]
  ) {
    suffix += 1;
  }

  const context = 240;
  const beforeEnd = before.length - suffix;
  const afterEnd = after.length - suffix;
  return {
    file_path: path,
    before: before.slice(Math.max(0, start - context), Math.min(before.length, beforeEnd + context)),
    after: after.slice(Math.max(0, start - context), Math.min(after.length, afterEnd + context)),
    cursor_line: Math.max(0, line - 1),
    cursor_column: Math.max(0, column - 1),
  };
}

export function DesktopApp() {
  const [apiToken, setApiToken] = useState(
    () => window.sessionStorage.getItem("openforge.apiToken") ?? "",
  );
  const client = useMemo(
    () => new OpenForgeClient("http://127.0.0.1:8765", apiToken || undefined),
    [apiToken],
  );

  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [caps, setCaps] = useState<CapabilitySet | null>(null);
  const [integrity, setIntegrity] = useState<EventIntegrityReport | null>(null);
  const [daemon, setDaemon] = useState<"checking" | "online" | "offline">("checking");
  const [error, setError] = useState("");

  const [repo, setRepo] = useState(
    () => window.localStorage.getItem("openforge.repository") ?? "",
  );
  const [repoInput, setRepoInput] = useState(repo);
  const [repository, setRepository] = useState<RepositoryIndex | null>(null);
  const [buffers, setBuffers] = useState<Record<string, BufferState>>({});
  const buffersRef = useRef(buffers);
  const [tabs, setTabs] = useState<string[]>([]);
  const [activePath, setActivePath] = useState("");
  const [secondaryPath, setSecondaryPath] = useState("");
  const [split, setSplit] = useState(false);
  const [focusedPane, setFocusedPane] = useState<"primary" | "secondary">("primary");
  const primaryEditorRef = useRef<MonacoEditor.IStandaloneCodeEditor | null>(null);
  const secondaryEditorRef = useRef<MonacoEditor.IStandaloneCodeEditor | null>(null);
  const monacoRef = useRef<Monaco | null>(null);

  const [sidebarView, setSidebarView] = useState<SidebarView>("explorer");
  const [toolView, setToolView] = useState<ToolView>("terminal");
  const [bottomOpen, setBottomOpen] = useState(true);
  const [fileFilter, setFileFilter] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [problems, setProblems] = useState<Problem[]>([]);
  const problemsRef = useRef(problems);
  const [activeRunId, setActiveRunId] = useState("");
  const activeRunRef = useRef(activeRunId);
  const [terminalCommand, setTerminalCommand] = useState("");
  const [terminalEpoch, setTerminalEpoch] = useState(0);

  const [lspServers, setLspServers] = useState<LanguageServerConfig[]>([]);
  const lspServersRef = useRef(lspServers);
  const lspSessionsRef = useRef(new Map<string, LspSession>());
  const languageSessionsRef = useRef(new Map<string, string>());
  const lspDisposablesRef = useRef(new Map<string, { dispose(): void }>());
  const predictionDisposablesRef = useRef(new Map<string, { dispose(): void }>());
  const semanticContextRef = useRef(new Map<string, string>());
  const recentEditsRef = useRef<RecentEdit[]>([]);

  useEffect(() => {
    buffersRef.current = buffers;
  }, [buffers]);
  useEffect(() => {
    problemsRef.current = problems;
  }, [problems]);
  useEffect(() => {
    activeRunRef.current = activeRunId;
  }, [activeRunId]);
  useEffect(() => {
    lspServersRef.current = lspServers;
  }, [lspServers]);

  const dirtyPaths = useMemo(
    () =>
      new Set(
        Object.values(buffers)
          .filter((buffer) => buffer.content !== buffer.savedContent)
          .map((buffer) => buffer.path),
      ),
    [buffers],
  );

  useEffect(() => {
    void invoke<SystemInfo>("system_info").then(setSystem);
    void Promise.all([
      client.initialize(),
      client.verifyEvents(),
      client.listLanguageServers(),
    ])
      .then(([capabilities, audit, servers]) => {
        setCaps(capabilities);
        setIntegrity(audit);
        setLspServers(servers);
        setDaemon("online");
        setError("");
      })
      .catch((failure: unknown) => {
        setDaemon("offline");
        setError(failure instanceof Error ? failure.message : String(failure));
      });
  }, [client]);

  useEffect(() => {
    if (!repo) return;
    void loadRepository(repo, false);
  }, [client]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      void collectDiagnostics();
    }, 700);
    return () => window.clearInterval(timer);
  }, [client, repo]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        const path = focusedPane === "secondary" ? secondaryPath : activePath;
        if (path) void saveFile(path);
      }
      if ((event.metaKey || event.ctrlKey) && event.key === "`") {
        event.preventDefault();
        setToolView("terminal");
        setBottomOpen(true);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [activePath, focusedPane, secondaryPath]);

  useEffect(() => {
    return () => {
      for (const session of lspSessionsRef.current.values()) {
        void client.stopLanguageServer(session.id);
      }
      for (const disposable of lspDisposablesRef.current.values()) disposable.dispose();
      for (const disposable of predictionDisposablesRef.current.values()) disposable.dispose();
    };
  }, [client]);

  async function loadRepository(path: string, checkDirty = true) {
    if (!path.trim()) return;
    if (
      checkDirty &&
      dirtyPaths.size > 0 &&
      !window.confirm("Open another repository and discard unsaved buffers?")
    ) {
      return;
    }
    try {
      const index = await client.repositoryIndex(path.trim());
      setRepository(index);
      setRepo(path.trim());
      setRepoInput(path.trim());
      window.localStorage.setItem("openforge.repository", path.trim());
      setBuffers({});
      setTabs([]);
      setActivePath("");
      setSecondaryPath("");
      setSearchHits([]);
      setProblems([]);
      semanticContextRef.current.clear();
      recentEditsRef.current = [];
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function chooseRepository() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "Open OpenForge repository",
    });
    if (typeof selected === "string") {
      await loadRepository(selected);
    }
  }

  function persistToken(value: string) {
    setApiToken(value);
    if (value) window.sessionStorage.setItem("openforge.apiToken", value);
    else window.sessionStorage.removeItem("openforge.apiToken");
  }

  async function ensurePrediction(language: string) {
    const monaco = monacoRef.current;
    if (!monaco || predictionDisposablesRef.current.has(language)) return;
    const disposable = registerEditPredictionProvider(monaco, client, language, {
      getRunId: () => activeRunRef.current,
      getPath: (uri) => {
        const path = relativeFromUri(uri, repo);
        return buffersRef.current[path] ? path : undefined;
      },
      getRecentEdits: () => recentEditsRef.current,
      getDiagnostics: (path) =>
        problemsRef.current
          .filter((problem) => problem.path === path)
          .map(toDiagnosticContext),
      getSemanticContext: (path) => semanticContextRef.current.get(path) ?? "",
    });
    predictionDisposablesRef.current.set(language, disposable);
  }

  async function ensureLsp(language: string): Promise<string | undefined> {
    const existing = languageSessionsRef.current.get(language);
    if (existing) return existing;

    const languageId = lspLanguageId(language);
    const config = lspServersRef.current.find(
      (server) =>
        server.languages.includes(language) || server.languages.includes(languageId),
    );
    if (!config) return undefined;

    let session = lspSessionsRef.current.get(config.name);
    if (!session) {
      const started = await client.startLanguageServer(config.name, repo);
      session = {
        id: started.session_id,
        config,
        capabilities: started.capabilities,
      };
      lspSessionsRef.current.set(config.name, session);
    }
    languageSessionsRef.current.set(language, session.id);

    const monaco = monacoRef.current;
    const key = language + ":" + session.id;
    if (monaco && !lspDisposablesRef.current.has(key)) {
      lspDisposablesRef.current.set(
        key,
        registerLspProviders(
          monaco,
          client,
          language,
          session.id,
          session.capabilities,
        ),
      );
    }
    return session.id;
  }

  async function openFile(path: string, pane = focusedPane) {
    if (!repo) return;
    const normalized = normalizePath(path);
    let buffer = buffersRef.current[normalized];

    if (!buffer) {
      try {
        const file = await client.readFile(repo, normalized);
        buffer = {
          path: normalized,
          content: file.content,
          savedContent: file.content,
          language: languageForPath(normalized),
          version: 1,
        };
        setBuffers((current) => ({ ...current, [normalized]: buffer! }));
        buffersRef.current = { ...buffersRef.current, [normalized]: buffer };
        setTabs((current) =>
          current.includes(normalized) ? current : [...current, normalized],
        );

        void client
          .searchKnowledge(repo, basename(normalized), 12)
          .then((value) => {
            semanticContextRef.current.set(
              normalized,
              value.symbols
                .map(
                  (symbol) =>
                    symbol.path +
                    ":" +
                    symbol.start_line +
                    " " +
                    symbol.kind +
                    " " +
                    symbol.signature,
                )
                .join("\n"),
            );
          })
          .catch(() => {});

        await ensurePrediction(buffer.language);
        const sessionId = await ensureLsp(buffer.language);
        if (sessionId) {
          await client.lspNotify(sessionId, "textDocument/didOpen", {
            textDocument: {
              uri: fileUri(repo, normalized),
              languageId: lspLanguageId(buffer.language),
              version: buffer.version,
              text: buffer.content,
            },
          });
        }
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : String(failure));
        return;
      }
    }

    if (pane === "secondary" && split) setSecondaryPath(normalized);
    else setActivePath(normalized);
  }

  async function saveFile(path: string) {
    const buffer = buffersRef.current[path];
    if (!buffer || buffer.content === buffer.savedContent) return;
    try {
      await client.writeFile(repo, path, buffer.content);
      setBuffers((current) => {
        const existing = current[path];
        if (!existing) return current;
        return {
          ...current,
          [path]: { ...existing, savedContent: existing.content },
        };
      });
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  function closeFile(path: string) {
    const buffer = buffersRef.current[path];
    if (
      buffer &&
      buffer.content !== buffer.savedContent &&
      !window.confirm("Close " + path + " without saving?")
    ) {
      return;
    }

    const sessionId = buffer
      ? languageSessionsRef.current.get(buffer.language)
      : undefined;
    if (sessionId) {
      void client.lspNotify(sessionId, "textDocument/didClose", {
        textDocument: { uri: fileUri(repo, path) },
      });
    }

    setTabs((current) => {
      const next = current.filter((item) => item !== path);
      if (activePath === path) setActivePath(next.at(-1) ?? "");
      if (secondaryPath === path) setSecondaryPath("");
      return next;
    });
    setBuffers((current) => {
      const next = { ...current };
      delete next[path];
      buffersRef.current = next;
      return next;
    });
    monacoRef.current
      ?.editor.getModel(monacoRef.current.Uri.parse(fileUri(repo, path)))
      ?.dispose();
  }

  function changeFile(
    path: string,
    value: string,
    editor: MonacoEditor.IStandaloneCodeEditor,
  ) {
    const existing = buffersRef.current[path];
    if (!existing || existing.content === value) return;

    const cursor = editor.getPosition();
    const edit = recentEdit(
      path,
      existing.content,
      value,
      cursor?.lineNumber ?? 1,
      cursor?.column ?? 1,
    );
    if (edit) {
      recentEditsRef.current = [edit, ...recentEditsRef.current].slice(0, 20);
    }

    const next = {
      ...existing,
      content: value,
      version: existing.version + 1,
    };
    buffersRef.current = { ...buffersRef.current, [path]: next };
    setBuffers((current) => ({ ...current, [path]: next }));

    const sessionId = languageSessionsRef.current.get(existing.language);
    if (sessionId) {
      void client.lspNotify(sessionId, "textDocument/didChange", {
        textDocument: {
          uri: fileUri(repo, path),
          version: next.version,
        },
        contentChanges: [{ text: value }],
      });
    }
  }

  async function collectDiagnostics() {
    const monaco = monacoRef.current;
    for (const session of lspSessionsRef.current.values()) {
      let notifications: unknown[];
      try {
        notifications = await client.lspNotifications(session.id);
      } catch {
        continue;
      }
      for (const notification of notifications) {
        if (
          typeof notification !== "object" ||
          notification === null ||
          !("method" in notification) ||
          (notification as { method?: unknown }).method !==
            "textDocument/publishDiagnostics"
        ) {
          continue;
        }
        const params = (notification as { params?: unknown }).params;
        if (typeof params !== "object" || params === null) continue;
        const typed = params as {
          uri?: unknown;
          diagnostics?: unknown;
        };
        if (
          typeof typed.uri !== "string" ||
          !Array.isArray(typed.diagnostics)
        ) {
          continue;
        }

        const path = relativeFromUri(typed.uri, repo);
        const nextProblems: Problem[] = typed.diagnostics.flatMap(
          (diagnostic): Problem[] => {
            if (typeof diagnostic !== "object" || diagnostic === null) return [];
            const value = diagnostic as {
              range?: {
                start?: { line?: number; character?: number };
                end?: { line?: number; character?: number };
              };
              severity?: number;
              message?: string;
              source?: string;
              code?: string | number;
            };
            if (!value.range?.start || !value.range.end || !value.message) return [];
            const problem: Problem = {
              uri: typed.uri as string,
              path,
              line: (value.range.start.line ?? 0) + 1,
              column: (value.range.start.character ?? 0) + 1,
              endLine: (value.range.end.line ?? value.range.start.line ?? 0) + 1,
              endColumn:
                (value.range.end.character ?? value.range.start.character ?? 0) + 1,
              severity: value.severity ?? 3,
              message: value.message,
            };
            if (value.source) problem.source = value.source;
            if (value.code !== undefined) problem.code = value.code;
            return [problem];
          },
        );

        setProblems((current) => {
          const merged = [
            ...current.filter((problem) => problem.uri !== typed.uri),
            ...nextProblems,
          ];
          problemsRef.current = merged;
          return merged;
        });

        if (monaco) {
          const model = monaco.editor.getModel(monaco.Uri.parse(typed.uri));
          if (model) {
            monaco.editor.setModelMarkers(
              model,
              "openforge-lsp",
              nextProblems.map((problem) => ({
                startLineNumber: problem.line,
                startColumn: problem.column,
                endLineNumber: problem.endLine,
                endColumn: problem.endColumn,
                severity:
                  problem.severity === 1
                    ? monaco.MarkerSeverity.Error
                    : problem.severity === 2
                      ? monaco.MarkerSeverity.Warning
                      : problem.severity === 3
                        ? monaco.MarkerSeverity.Info
                        : monaco.MarkerSeverity.Hint,
                message: problem.message,
                ...(problem.source !== undefined ? { source: problem.source } : {}),
                ...(problem.code !== undefined ? { code: problem.code } : {}),
              })),
            );
          }
        }
      }
    }
  }

  async function searchRepository() {
    if (!repo || !searchQuery.trim()) return;
    try {
      const result = await client.search(repo, searchQuery.trim(), 100);
      setSearchHits(result.hits);
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  const onMount: OnMount = (editor, monaco) => {
    if (!monacoRef.current) monacoRef.current = monaco;
    editor.onDidFocusEditorText(() => {
      if (editor === secondaryEditorRef.current) setFocusedPane("secondary");
      else setFocusedPane("primary");
    });
  };

  function runCommand(command: string) {
    setTerminalCommand(command);
    setTerminalEpoch((value) => value + 1);
    setToolView("terminal");
    setBottomOpen(true);
  }

  async function revealProblem(problem: Problem) {
    await openFile(problem.path, "primary");
    setFocusedPane("primary");
    window.setTimeout(() => {
      primaryEditorRef.current?.setPosition({
        lineNumber: problem.line,
        column: problem.column,
      });
      primaryEditorRef.current?.revealLineInCenter(problem.line);
      primaryEditorRef.current?.focus();
    }, 50);
  }

  const active = buffers[activePath];
  const secondary = buffers[secondaryPath];

  function editorPane(
    buffer: BufferState | undefined,
    pane: "primary" | "secondary",
  ) {
    if (!buffer) {
      return (
        <div
          className={"editor-empty" + (focusedPane === pane ? " focused" : "")}
          onMouseDown={() => setFocusedPane(pane)}
        >
          <div className="openforge-mark">OF</div>
          <h2>OpenForge</h2>
          <p>Open a file from Explorer or repository search.</p>
        </div>
      );
    }

    return (
      <div className={"editor-instance" + (focusedPane === pane ? " focused" : "")}>
        <Editor
          theme="vs-dark"
          path={fileUri(repo, buffer.path)}
          language={buffer.language}
          value={buffer.content}
          keepCurrentModel
          saveViewState
          onMount={(editor, monaco) => {
            if (pane === "primary") primaryEditorRef.current = editor;
            else secondaryEditorRef.current = editor;
            onMount(editor, monaco);
            void ensurePrediction(buffer.language);
            void ensureLsp(buffer.language);
          }}
          onChange={(value) => {
            const editor =
              pane === "primary"
                ? primaryEditorRef.current
                : secondaryEditorRef.current;
            if (editor) changeFile(buffer.path, value ?? "", editor);
          }}
          options={{
            automaticLayout: true,
            minimap: { enabled: true },
            bracketPairColorization: { enabled: true },
            guides: {
              bracketPairs: true,
              indentation: true,
            },
            multiCursorModifier: "alt",
            codeLens: true,
            formatOnPaste: false,
            formatOnType: false,
            fontFamily:
              "JetBrains Mono, SFMono-Regular, Consolas, Liberation Mono, monospace",
            fontSize: 13,
            lineHeight: 20,
            smoothScrolling: true,
            cursorBlinking: "smooth",
            renderWhitespace: "selection",
            wordWrap: "off",
            scrollBeyondLastLine: false,
            stickyScroll: { enabled: true },
          }}
        />
      </div>
    );
  }

  return (
    <main className="ide-shell">
      <header className="titlebar">
        <div className="brand">
          <strong>OpenForge</strong>
          <span>0.3 IDE</span>
        </div>
        <div className="repo-picker">
          <button onClick={() => void chooseRepository()}>Open</button>
          <input
            value={repoInput}
            onChange={(event) => setRepoInput(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void loadRepository(repoInput);
            }}
            placeholder="/path/to/repository"
          />
          <button onClick={() => void loadRepository(repoInput)}>Load</button>
        </div>
        <div className="titlebar-status">
          <span className={"daemon-dot " + daemon} />
          <span>{daemon}</span>
          <span>{system ? system.platform + "/" + system.arch : ""}</span>
          <span>{integrity?.valid ? "audit ✓" : integrity ? "audit !" : ""}</span>
          <input
            className="token-input"
            type="password"
            value={apiToken}
            onChange={(event) => persistToken(event.target.value)}
            placeholder="API token"
            aria-label="OpenForge API token"
          />
        </div>
      </header>

      <div className="ide-body">
        <aside className="activity-bar">
          {([
            ["explorer", "▱", "Explorer"],
            ["search", "⌕", "Search"],
            ["source-control", "⑂", "Source Control"],
            ["agents", "◇", "Agents"],
          ] as Array<[SidebarView, string, string]>).map(([view, icon, title]) => (
            <button
              key={view}
              className={sidebarView === view ? "active" : ""}
              onClick={() => setSidebarView(view)}
              title={title}
            >
              {icon}
            </button>
          ))}
        </aside>

        <aside className="sidebar">
          {sidebarView === "explorer" && (
            <>
              <div className="sidebar-title">
                <span>Explorer</span>
                <span>{repository?.files.length ?? 0}</span>
              </div>
              <input
                className="sidebar-filter"
                value={fileFilter}
                onChange={(event) => setFileFilter(event.target.value)}
                placeholder="Filter files"
              />
              <FileTree
                files={repository?.files ?? []}
                activePath={focusedPane === "secondary" ? secondaryPath : activePath}
                dirtyPaths={dirtyPaths}
                filter={fileFilter}
                onOpen={(path) => void openFile(path)}
              />
            </>
          )}

          {sidebarView === "search" && (
            <>
              <div className="sidebar-title">Repository Search</div>
              <div className="sidebar-search">
                <input
                  value={searchQuery}
                  onChange={(event) => setSearchQuery(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void searchRepository();
                  }}
                  placeholder="Code, symbols, concepts"
                />
                <button onClick={() => void searchRepository()}>Search</button>
              </div>
              <div className="search-results">
                {searchHits.map((hit) => (
                  <button
                    key={hit.path + ":" + hit.line + ":" + hit.score}
                    onClick={() => void openFile(hit.path)}
                  >
                    <strong>
                      {hit.path}:{hit.line}
                    </strong>
                    <span>{hit.snippet}</span>
                  </button>
                ))}
              </div>
            </>
          )}

          {sidebarView === "source-control" && repo && (
            <SourceControlPane
              client={client}
              repo={repo}
              onOpenFile={(path) => void openFile(path)}
            />
          )}

          {sidebarView === "agents" && repo && (
            <AgentPane
              client={client}
              repo={repo}
              onRunChange={setActiveRunId}
            />
          )}
        </aside>

        <section className="workbench">
          <div className="editor-tabs">
            <div className="tabs-scroll">
              {tabs.map((path) => (
                <button
                  className={
                    "editor-tab" +
                    ((focusedPane === "primary" && activePath === path) ||
                    (focusedPane === "secondary" && secondaryPath === path)
                      ? " active"
                      : "")
                  }
                  key={path}
                  onClick={() => {
                    if (focusedPane === "secondary" && split) setSecondaryPath(path);
                    else setActivePath(path);
                  }}
                  title={path}
                >
                  <span>{basename(path)}</span>
                  {dirtyPaths.has(path) && <span className="tab-dirty">●</span>}
                  <span
                    className="tab-close"
                    role="button"
                    tabIndex={0}
                    onClick={(event) => {
                      event.stopPropagation();
                      closeFile(path);
                    }}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") closeFile(path);
                    }}
                  >
                    ×
                  </span>
                </button>
              ))}
            </div>
            <button
              className="split-button"
              onClick={() => {
                if (split) {
                  setSplit(false);
                  setSecondaryPath("");
                  setFocusedPane("primary");
                } else {
                  setSplit(true);
                  const candidate =
                    tabs.find((path) => path !== activePath) ?? activePath;
                  setSecondaryPath(candidate);
                  setFocusedPane("secondary");
                }
              }}
              title="Toggle split editor"
            >
              {split ? "▣" : "▥"}
            </button>
          </div>

          <div className="breadcrumbs">
            <span>{repo ? basename(repo) : "No repository"}</span>
            {(focusedPane === "secondary" ? secondaryPath : activePath)
              .split("/")
              .filter(Boolean)
              .map((part) => (
                <span key={part}>{part}</span>
              ))}
          </div>

          <div className={"editors" + (split ? " split" : "")}>
            {editorPane(active, "primary")}
            {split && editorPane(secondary, "secondary")}
          </div>

          {bottomOpen && (
            <div className="bottom-panel">
              <div className="tool-tabs">
                {([
                  ["terminal", "Terminal"],
                  ["problems", "Problems " + problems.length],
                  ["tests", "Tests"],
                  ["debug", "Debug"],
                  ["browser", "Browser"],
                  ["database", "Database"],
                  ["output", "Output"],
                ] as Array<[ToolView, string]>).map(([view, title]) => (
                  <button
                    key={view}
                    className={toolView === view ? "active" : ""}
                    onClick={() => setToolView(view)}
                  >
                    {title}
                  </button>
                ))}
                <button
                  className="panel-close"
                  onClick={() => setBottomOpen(false)}
                  title="Close panel"
                >
                  ×
                </button>
              </div>
              <div className="tool-content">
                {toolView === "terminal" && repo && system && (
                  <TerminalPane
                    key={repo + ":" + terminalEpoch}
                    client={client}
                    repo={repo}
                    platform={system.platform}
                    initialCommand={terminalCommand}
                  />
                )}
                {toolView === "problems" && (
                  <ProblemsPane
                    problems={problems}
                    onOpen={(problem) => void revealProblem(problem)}
                  />
                )}
                {toolView === "tests" && repo && (
                  <TestExplorerPane
                    client={client}
                    repo={repo}
                    onOpenFile={(path) => void openFile(path)}
                    onRunCommand={runCommand}
                  />
                )}
                {toolView === "debug" && repo && (
                  <DebugPane client={client} repo={repo} />
                )}
                {toolView === "browser" && <BrowserPane client={client} />}
                {toolView === "database" && repo && (
                  <DatabasePane client={client} repo={repo} />
                )}
                {toolView === "output" && (
                  <pre className="output-pane">
                    {error ||
                      JSON.stringify(
                        {
                          protocol: caps?.protocol_version,
                          daemon: caps?.server_version,
                          repository: repository
                            ? {
                                fingerprint: repository.fingerprint,
                                files: repository.files.length,
                                lines: repository.total_lines,
                              }
                            : null,
                          lsp_sessions: [...lspSessionsRef.current.entries()].map(
                            ([name, session]) => ({
                              name,
                              id: session.id,
                            }),
                          ),
                          active_run: activeRunId || null,
                        },
                        null,
                        2,
                      )}
                  </pre>
                )}
              </div>
            </div>
          )}

          <footer className="statusbar">
            <button onClick={() => setBottomOpen((value) => !value)}>
              {bottomOpen ? "Panel ↓" : "Panel ↑"}
            </button>
            <span>{repo || "No repository"}</span>
            <span>{dirtyPaths.size ? dirtyPaths.size + " unsaved" : "saved"}</span>
            <span>{problems.filter((problem) => problem.severity === 1).length} errors</span>
            <span>{problems.filter((problem) => problem.severity === 2).length} warnings</span>
            <span>{active?.language ?? "plaintext"}</span>
            <span>{activeRunId ? "run " + activeRunId.slice(0, 8) : "no active run"}</span>
          </footer>
        </section>
      </div>
    </main>
  );
}
