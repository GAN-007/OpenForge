import * as vscode from "vscode";

type Task = {
  id: string;
  title: string;
  role: string;
  status: string;
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
    if (token) {
      headers.authorization = "Bearer " + token;
    }
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
      throw new Error(
        body.error?.message ?? "HTTP " + response.status,
      );
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
    item.tooltip = task.id;
    item.iconPath = new vscode.ThemeIcon(
      task.status === "completed"
        ? "pass"
        : task.status === "failed"
          ? "error"
          : "gear",
    );
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
      ? await this.rpc.call<Task[]>("task/list", {
          run_id: runId,
        })
      : [];
    this.refresh();
  }
}

class CompletionProvider
  implements vscode.InlineCompletionItemProvider
{
  private lastRequestAt = 0;
  private lastKey = "";
  private lastValue = "";

  constructor(private rpc: Rpc) {}

  async provideInlineCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position,
    _context: vscode.InlineCompletionContext,
    token: vscode.CancellationToken,
  ): Promise<vscode.InlineCompletionItem[]> {
    const config = vscode.workspace.getConfiguration("openforge");
    if (!config.get<boolean>("inlineCompletion.enabled", true)) {
      return [];
    }

    const runId = config.get<string>("runId", "").trim();
    if (!runId) return [];

    const now = Date.now();
    if (now - this.lastRequestAt < 250) {
      return [];
    }
    this.lastRequestAt = now;

    const cursorOffset = document.offsetAt(position);
    const text = document.getText();
    const prefix = text.slice(Math.max(0, cursorOffset - 6000), cursorOffset);
    const suffix = text.slice(cursorOffset, cursorOffset + 2000);
    const key = [
      document.uri.toString(),
      document.version,
      cursorOffset,
      prefix.slice(-160),
      suffix.slice(0, 160),
    ].join("|");

    if (key === this.lastKey && this.lastValue) {
      return [new vscode.InlineCompletionItem(this.lastValue)];
    }

    const controller = new AbortController();
    const disposable = token.onCancellationRequested(() =>
      controller.abort(),
    );

    try {
      const response = await this.rpc.call<{ text: string }>(
        "completion/request",
        {
          run_id: runId,
          file_path:
            vscode.workspace.asRelativePath(document.uri, false),
          language: document.languageId,
          prefix,
          suffix,
          max_output_tokens: 256,
          max_cost_usd: config.get<number>(
            "inlineCompletion.maxCostUsd",
            0.03,
          ),
        },
        controller.signal,
      );

      if (token.isCancellationRequested || !response.text) {
        return [];
      }

      this.lastKey = key;
      this.lastValue = response.text;
      return [new vscode.InlineCompletionItem(response.text)];
    } catch (error) {
      if (!controller.signal.aborted) {
        console.warn("OpenForge completion failed", error);
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

  context.subscriptions.push(
    vscode.window.registerTreeDataProvider(
      "openforge.tasks",
      tasks,
    ),
  );

  context.subscriptions.push(
    vscode.languages.registerInlineCompletionItemProvider(
      { pattern: "**" },
      new CompletionProvider(rpc),
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand(
      "openforge.connect",
      async () => {
        try {
          const capabilities =
            await rpc.call<{ server_version: string }>(
              "initialize",
            );
          void vscode.window.showInformationMessage(
            "OpenForge daemon " +
              capabilities.server_version +
              " connected",
          );
        } catch (error) {
          void vscode.window.showErrorMessage(String(error));
        }
      },
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand(
      "openforge.setApiToken",
      async () => {
        const token = await vscode.window.showInputBox({
          prompt: "OpenForge daemon API token",
          password: true,
          ignoreFocusOut: true,
          placeHolder: "Leave blank to clear the stored token",
        });
        if (token === undefined) return;
        if (token.trim()) {
          await context.secrets.store("openforge.apiToken", token.trim());
          void vscode.window.showInformationMessage(
            "OpenForge API token stored securely for this VS Code profile.",
          );
        } else {
          await context.secrets.delete("openforge.apiToken");
          void vscode.window.showInformationMessage(
            "OpenForge API token cleared.",
          );
        }
      },
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand(
      "openforge.verifyAudit",
      async () => {
        try {
          const report = await rpc.call<{
            valid: boolean;
            verified_events: number;
            violations: string[];
          }>("event/verify");
          if (report.valid) {
            void vscode.window.showInformationMessage(
              `OpenForge audit chain valid across ${report.verified_events} events.`,
            );
          } else {
            void vscode.window.showErrorMessage(
              "OpenForge audit chain failed verification: " +
                report.violations.slice(0, 3).join("; "),
            );
          }
        } catch (error) {
          void vscode.window.showErrorMessage(String(error));
        }
      },
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand(
      "openforge.searchRepository",
      async () => {
        const workspace = vscode.workspace.workspaceFolders?.[0];
        if (!workspace) {
          void vscode.window.showWarningMessage(
            "Open a workspace before searching repository intelligence.",
          );
          return;
        }
        const query = await vscode.window.showInputBox({
          prompt: "Search repository content and symbols",
          placeHolder: "authentication policy scheduler",
        });
        if (!query?.trim()) return;

        try {
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
              label: `${hit.path}:${hit.line}`,
              description: `score ${hit.score.toFixed(2)}`,
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
        } catch (error) {
          void vscode.window.showErrorMessage(String(error));
        }
      },
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand(
      "openforge.refresh",
      () => tasks.load(),
    ),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand(
      "openforge.inspectRun",
      async () => {
        const id = await vscode.window.showInputBox({
          prompt: "OpenForge run UUID",
        });
        if (!id) return;
        await vscode.workspace
          .getConfiguration("openforge")
          .update(
            "runId",
            id,
            vscode.ConfigurationTarget.Workspace,
          );
        await tasks.load();
      },
    ),
  );

  void tasks.load();
}

export function deactivate() {}
