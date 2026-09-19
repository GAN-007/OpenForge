import * as vscode from "vscode";

type Task = {
  id: string;
  title: string;
  description: string;
  role: string;
  status: string;
  dependencies: string[];
  attempts: number;
  max_attempts: number;
};

type RecentEdit = {
  file_path: string;
  before: string;
  after: string;
  cursor_line: number;
  cursor_column: number;
};

type DiagnosticContext = {
  file_path: string;
  line: number;
  column: number;
  severity: string;
  message: string;
};

type PredictedEdit = {
  file_path: string;
  range: {
    start_line: number;
    start_column: number;
    end_line: number;
    end_column: number;
  };
  new_text: string;
};

type CursorTarget = {
  file_path: string;
  line: number;
  column: number;
};

type EditPrediction = {
  edits: PredictedEdit[];
  next_cursor?: CursorTarget | null;
  confidence: number;
  provider: string;
  model: string;
  latency_ms: number;
  cost_usd: number;
  cache_hit: boolean;
};

class Rpc {
  private id = 0;

  constructor(
    private base: string,
    private tokenProvider?: () => Thenable<string | undefined>,
  ) {}

  async call<T>(
    method: string,
    params: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<T> {
    const headers: Record<string, string> = {
      "content-type": "application/json",
    };
    const token = await this.tokenProvider?.();
    if (token) headers.authorization = "Bearer " + token;

    const response = await fetch(this.base + "/v1/rpc", {
      method: "POST",
      headers,
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: ++this.id,
        method,
        params,
      }),
      signal: signal ?? null,
    });
    const body = (await response.json()) as {
      result?: T;
      error?: { message: string };
    };
    if (!response.ok || body.error) {
      throw new Error(body.error?.message ?? "HTTP " + response.status);
    }
    if (body.result === undefined) {
      throw new Error("OpenForge daemon returned no result");
    }
    return body.result;
  }
}

class TasksProvider implements vscode.TreeDataProvider<Task> {
  private changed = new vscode.EventEmitter<Task | undefined | void>();
  readonly onDidChangeTreeData = this.changed.event;
  tasks: Task[] = [];

  constructor(private rpc: Rpc) {}

  refresh() {
    this.changed.fire();
  }

  getTreeItem(task: Task) {
    const item = new vscode.TreeItem(task.title);
    item.description = task.role + " · " + task.status;
    item.tooltip =
      task.description +
      "\n\nRole: " +
      task.role +
      "\nStatus: " +
      task.status +
      "\nAttempts: " +
      task.attempts +
      "/" +
      task.max_attempts;
    item.iconPath = new vscode.ThemeIcon(
      task.status === "completed"
        ? "pass"
        : task.status === "failed"
          ? "error"
          : task.status === "running" || task.status === "reviewing"
            ? "sync~spin"
            : "circle-outline",
    );
    item.command = {
      command: "openforge.showTask",
      title: "Show OpenForge Task",
      arguments: [task],
    };
    return item;
  }

  getChildren() {
    return this.tasks;
  }

  async load() {
    const runId = vscode.workspace
      .getConfiguration("openforge")
      .get<string>("runId", "")
      .trim();
    this.tasks = runId
      ? await this.rpc.call<Task[]>("task/list", { run_id: runId })
      : [];
    this.refresh();
  }
}

class EditPredictionProvider implements vscode.InlineCompletionItemProvider {
  private lastRequestAt = 0;
  private snapshots = new Map<string, string>();
  private recentEdits: RecentEdit[] = [];
  private semanticCache = new Map<string, string>();

  constructor(private rpc: Rpc) {
    for (const document of vscode.workspace.textDocuments) {
      this.snapshots.set(document.uri.toString(), document.getText());
    }
  }

  recordOpen(document: vscode.TextDocument) {
    this.snapshots.set(document.uri.toString(), document.getText());
  }

  recordClose(document: vscode.TextDocument) {
    this.snapshots.delete(document.uri.toString());
  }

  recordChange(event: vscode.TextDocumentChangeEvent) {
    const key = event.document.uri.toString();
    const before = this.snapshots.get(key) ?? "";
    const after = event.document.getText();
    this.snapshots.set(key, after);
    if (before === after) return;

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

    const cursorOffset = Math.min(
      after.length,
      start + Math.max(0, after.length - suffix - start),
    );
    const cursor = event.document.positionAt(cursorOffset);
    const workspace = vscode.workspace.getWorkspaceFolder(event.document.uri);
    if (!workspace) return;

    const context = 240;
    this.recentEdits.unshift({
      file_path: vscode.workspace.asRelativePath(event.document.uri, false),
      before: before.slice(
        Math.max(0, start - context),
        Math.min(before.length, before.length - suffix + context),
      ),
      after: after.slice(
        Math.max(0, start - context),
        Math.min(after.length, after.length - suffix + context),
      ),
      cursor_line: cursor.line,
      cursor_column: cursor.character,
    });
    this.recentEdits = this.recentEdits.slice(0, 20);
  }

  private async semanticContext(
    workspace: vscode.WorkspaceFolder,
    document: vscode.TextDocument,
  ) {
    const key = workspace.uri.fsPath + "::" + document.uri.fsPath;
    const cached = this.semanticCache.get(key);
    if (cached !== undefined) return cached;

    try {
      const name = document.uri.path.split("/").at(-1) ?? document.fileName;
      const result = await this.rpc.call<{
        symbols: Array<{
          path: string;
          start_line: number;
          kind: string;
          signature: string;
        }>;
      }>("knowledge/search", {
        repo: workspace.uri.fsPath,
        query: name,
        limit: 12,
      });
      const value = result.symbols
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
        .join("\n");
      this.semanticCache.set(key, value);
      return value;
    } catch {
      return "";
    }
  }

  async provideInlineCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position,
    _context: vscode.InlineCompletionContext,
    token: vscode.CancellationToken,
  ): Promise<vscode.InlineCompletionItem[]> {
    const config = vscode.workspace.getConfiguration("openforge");
    if (!config.get<boolean>("inlineCompletion.enabled", true)) return [];

    const runId = config.get<string>("runId", "").trim();
    if (!runId) return [];
    const workspace = vscode.workspace.getWorkspaceFolder(document.uri);
    if (!workspace) return [];

    const now = Date.now();
    if (now - this.lastRequestAt < 180) return [];
    this.lastRequestAt = now;

    const controller = new AbortController();
    const disposable = token.onCancellationRequested(() => controller.abort());

    try {
      const text = document.getText();
      const cursorOffset = document.offsetAt(position);
      const filePath = vscode.workspace.asRelativePath(document.uri, false);
      const diagnostics: DiagnosticContext[] = vscode.languages
        .getDiagnostics(document.uri)
        .slice(0, 50)
        .map((diagnostic) => ({
          file_path: filePath,
          line: diagnostic.range.start.line,
          column: diagnostic.range.start.character,
          severity: vscode.DiagnosticSeverity[
            diagnostic.severity
          ].toLowerCase(),
          message: diagnostic.message,
        }));

      const prediction = await this.rpc.call<EditPrediction>(
        "edit/predict",
        {
          run_id: runId,
          file_path: filePath,
          language: document.languageId,
          prefix: text.slice(0, cursorOffset),
          suffix: text.slice(cursorOffset),
          recent_edits: this.recentEdits.slice(0, 12),
          diagnostics,
          semantic_context: await this.semanticContext(workspace, document),
          max_cost_usd: config.get<number>(
            "inlineCompletion.maxCostUsd",
            0.03,
          ),
          max_latency_ms: config.get<number>(
            "inlineCompletion.maxLatencyMs",
            1500,
          ),
        },
        controller.signal,
      );

      if (
        token.isCancellationRequested ||
        prediction.confidence <
          config.get<number>("inlineCompletion.minConfidence", 0.35)
      ) {
        return [];
      }

      return prediction.edits
        .filter((edit) => edit.file_path === filePath)
        .map(
          (edit) =>
            new vscode.InlineCompletionItem(
              edit.new_text,
              new vscode.Range(
                new vscode.Position(
                  edit.range.start_line,
                  edit.range.start_column,
                ),
                new vscode.Position(
                  edit.range.end_line,
                  edit.range.end_column,
                ),
              ),
            ),
        );
    } catch (error) {
      if (!controller.signal.aborted) {
        console.debug("OpenForge edit prediction skipped", error);
      }
      return [];
    } finally {
      disposable.dispose();
    }
  }
}

export function activate(context: vscode.ExtensionContext) {
  const url = vscode.workspace
    .getConfiguration("openforge")
    .get<string>("daemonUrl", "http://127.0.0.1:8765");
  const rpc = new Rpc(
    url,
    () => context.secrets.get("openforge.apiToken"),
  );
  const tasks = new TasksProvider(rpc);
  const predictions = new EditPredictionProvider(rpc);
  const output = vscode.window.createOutputChannel("OpenForge", { log: true });
  const status = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    40,
  );
  status.command = "openforge.inspectRun";
  status.show();

  const updateStatus = () => {
    const runId = vscode.workspace
      .getConfiguration("openforge")
      .get<string>("runId", "")
      .trim();
    status.text = runId
      ? "$(hubot) OpenForge " + runId.slice(0, 8)
      : "$(hubot) OpenForge";
    status.tooltip = runId
      ? "Current OpenForge run: " + runId
      : "No OpenForge run selected";
  };
  updateStatus();

  context.subscriptions.push(output, status);
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("openforge.tasks", tasks),
    vscode.languages.registerInlineCompletionItemProvider(
      { pattern: "**" },
      predictions,
    ),
    vscode.workspace.onDidOpenTextDocument((document) =>
      predictions.recordOpen(document),
    ),
    vscode.workspace.onDidCloseTextDocument((document) =>
      predictions.recordClose(document),
    ),
    vscode.workspace.onDidChangeTextDocument((event) =>
      predictions.recordChange(event),
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("openforge.showTask", (task: Task) => {
      output.show(true);
      output.info(JSON.stringify(task, null, 2));
    }),
    vscode.commands.registerCommand("openforge.connect", async () => {
      try {
        const capabilities =
          await rpc.call<{ server_version: string; protocol_version: string }>(
            "initialize",
          );
        void vscode.window.showInformationMessage(
          "OpenForge " +
            capabilities.server_version +
            " · " +
            capabilities.protocol_version +
            " connected",
        );
      } catch (error) {
        void vscode.window.showErrorMessage(String(error));
      }
    }),
    vscode.commands.registerCommand("openforge.setApiToken", async () => {
      const token = await vscode.window.showInputBox({
        prompt: "OpenForge daemon API/team token",
        password: true,
        ignoreFocusOut: true,
        placeHolder: "Leave blank to clear the stored token",
      });
      if (token === undefined) return;
      if (token.trim()) {
        await context.secrets.store("openforge.apiToken", token.trim());
      } else {
        await context.secrets.delete("openforge.apiToken");
      }
    }),
    vscode.commands.registerCommand("openforge.createRun", async () => {
      const workspace = vscode.workspace.workspaceFolders?.[0];
      if (!workspace) {
        void vscode.window.showWarningMessage(
          "Open a workspace before creating an OpenForge run.",
        );
        return;
      }
      const objective = await vscode.window.showInputBox({
        prompt: "Engineering objective",
        ignoreFocusOut: true,
      });
      if (!objective?.trim()) return;

      const autonomy =
        (await vscode.window.showQuickPick(
          ["observe", "suggest", "edit", "execute", "autonomous"],
          {
            title: "OpenForge autonomy",
            placeHolder: "Select an autonomy ceiling",
          },
        )) ?? "edit";
      const budgetRaw = await vscode.window.showInputBox({
        prompt: "Run hard budget (USD)",
        value: "5",
        validateInput: (value) => {
          const parsed = Number(value);
          return Number.isFinite(parsed) && parsed > 0
            ? undefined
            : "Enter a positive number";
        },
      });
      if (budgetRaw === undefined) return;

      try {
        const run = await rpc.call<{ id: string }>("run/create", {
          repo: workspace.uri.fsPath,
          objective: objective.trim(),
          autonomy,
          budget_usd: Number(budgetRaw),
        });
        await rpc.call("run/plan", {
          repo: workspace.uri.fsPath,
          run_id: run.id,
        });
        await vscode.workspace
          .getConfiguration("openforge")
          .update(
            "runId",
            run.id,
            vscode.ConfigurationTarget.Workspace,
          );
        updateStatus();
        await tasks.load();
        void vscode.window.showInformationMessage(
          "OpenForge run " + run.id.slice(0, 8) + " planned.",
        );
      } catch (error) {
        void vscode.window.showErrorMessage(String(error));
      }
    }),
    vscode.commands.registerCommand("openforge.executeRun", async () => {
      const workspace = vscode.workspace.workspaceFolders?.[0];
      const runId = vscode.workspace
        .getConfiguration("openforge")
        .get<string>("runId", "")
        .trim();
      if (!workspace || !runId) {
        void vscode.window.showWarningMessage(
          "Select an OpenForge run before execution.",
        );
        return;
      }

      await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: "OpenForge executing " + runId.slice(0, 8),
          cancellable: false,
        },
        async () => {
          const timer = setInterval(() => void tasks.load(), 1200);
          try {
            const result = await rpc.call<{ integration_branch: string }>(
              "run/execute",
              {
                repo: workspace.uri.fsPath,
                run_id: runId,
                docker: false,
              },
            );
            output.info(
              "Run completed on integration branch " +
                result.integration_branch,
            );
          } finally {
            clearInterval(timer);
            await tasks.load();
          }
        },
      );
    }),
    vscode.commands.registerCommand(
      "openforge.queueInstruction",
      async () => {
        const threadId = await vscode.window.showInputBox({
          prompt: "OpenForge agent thread UUID",
        });
        if (!threadId?.trim()) return;
        const instruction = await vscode.window.showInputBox({
          prompt: "Instruction to queue for the running agent",
          ignoreFocusOut: true,
        });
        if (!instruction?.trim()) return;
        await rpc.call("thread/instruction-queue", {
          thread_id: threadId.trim(),
          content: instruction.trim(),
        });
      },
    ),
    vscode.commands.registerCommand("openforge.verifyAudit", async () => {
      try {
        const report = await rpc.call<{
          valid: boolean;
          verified_events: number;
          violations: string[];
        }>("event/verify");
        if (report.valid) {
          void vscode.window.showInformationMessage(
            "OpenForge audit chain valid across " +
              report.verified_events +
              " events.",
          );
        } else {
          void vscode.window.showErrorMessage(
            "OpenForge audit chain failed: " +
              report.violations.slice(0, 3).join("; "),
          );
        }
      } catch (error) {
        void vscode.window.showErrorMessage(String(error));
      }
    }),
    vscode.commands.registerCommand(
      "openforge.searchRepository",
      async () => {
        const workspace = vscode.workspace.workspaceFolders?.[0];
        if (!workspace) return;
        const query = await vscode.window.showInputBox({
          prompt: "Search repository content, symbols and concepts",
        });
        if (!query?.trim()) return;

        const result = await rpc.call<{
          hits: Array<{
            path: string;
            line: number;
            snippet: string;
            score: number;
          }>;
        }>("search/query", {
          repo: workspace.uri.fsPath,
          query: query.trim(),
          limit: 50,
        });
        const selected = await vscode.window.showQuickPick(
          result.hits.map((hit) => ({
            label: hit.path + ":" + hit.line,
            description: "score " + hit.score.toFixed(2),
            detail: hit.snippet,
            hit,
          })),
          {
            placeHolder: result.hits.length
              ? "Select a repository hit"
              : "No repository hits found",
            matchOnDescription: true,
            matchOnDetail: true,
          },
        );
        if (!selected) return;
        const uri = vscode.Uri.joinPath(workspace.uri, selected.hit.path);
        const document = await vscode.workspace.openTextDocument(uri);
        const editor = await vscode.window.showTextDocument(document);
        const line = Math.max(0, selected.hit.line - 1);
        const position = new vscode.Position(line, 0);
        editor.selection = new vscode.Selection(position, position);
        editor.revealRange(
          new vscode.Range(position, position),
          vscode.TextEditorRevealType.InCenter,
        );
      },
    ),
    vscode.commands.registerCommand("openforge.refresh", () => tasks.load()),
    vscode.commands.registerCommand("openforge.inspectRun", async () => {
      const id = await vscode.window.showInputBox({
        prompt: "OpenForge run UUID",
        value: vscode.workspace
          .getConfiguration("openforge")
          .get<string>("runId", ""),
      });
      if (!id?.trim()) return;
      await vscode.workspace
        .getConfiguration("openforge")
        .update(
          "runId",
          id.trim(),
          vscode.ConfigurationTarget.Workspace,
        );
      updateStatus();
      await tasks.load();
    }),
  );

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("openforge.runId")) {
        updateStatus();
        void tasks.load();
      }
    }),
  );

  void tasks.load();
}

export function deactivate() {}
