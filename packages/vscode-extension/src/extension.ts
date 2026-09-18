import * as vscode from "vscode";

type Task = {
  id: string;
  title: string;
  role: string;
  status: string;
};

class Rpc {
  private id = 0;

  constructor(private base: string) {}

  async call<T>(
    method: string,
    params: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<T> {
    const response = await fetch(this.base + "/v1/rpc", {
      method: "POST",
      headers: { "content-type": "application/json" },
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

  const rpc = new Rpc(url);
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
