import type { Monaco } from "@monaco-editor/react";
import type { OpenForgeClient } from "@openforge/sdk";

type LspPosition = { line: number; character: number };
type LspRange = { start: LspPosition; end: LspPosition };
type LspLocation = { uri: string; range: LspRange };
type LspLocationLink = {
  targetUri: string;
  targetRange: LspRange;
  targetSelectionRange: LspRange;
};
type LspTextEdit = { range: LspRange; newText: string };
type LspWorkspaceEdit = {
  changes?: Record<string, LspTextEdit[]>;
};
type LspCompletionItem = {
  label: string;
  detail?: string;
  documentation?: string | { value?: string };
  kind?: number;
  insertText?: string;
  insertTextFormat?: number;
  textEdit?: LspTextEdit;
};
type LspHover = {
  contents:
    | string
    | { value?: string }
    | Array<string | { value?: string }>;
  range?: LspRange;
};
type LspCodeAction = {
  title: string;
  kind?: string;
  isPreferred?: boolean;
  edit?: LspWorkspaceEdit;
};
type LspSignatureHelp = {
  signatures: Array<{
    label: string;
    documentation?: string | { value?: string };
    parameters?: Array<{
      label: string | [number, number];
      documentation?: string | { value?: string };
    }>;
  }>;
  activeSignature?: number;
  activeParameter?: number;
};
type LspDocumentSymbol = {
  name: string;
  detail?: string;
  kind: number;
  range: LspRange;
  selectionRange: LspRange;
  children?: LspDocumentSymbol[];
};

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}

function range(monaco: Monaco, value: LspRange) {
  return new monaco.Range(
    value.start.line + 1,
    value.start.character + 1,
    value.end.line + 1,
    value.end.character + 1,
  );
}

function documentation(value: unknown): string {
  if (typeof value === "string") return value;
  const object = record(value);
  return typeof object?.value === "string" ? object.value : "";
}

function hoverContents(value: LspHover["contents"]): string {
  if (Array.isArray(value)) {
    return value.map(documentation).filter(Boolean).join("\n\n");
  }
  return documentation(value);
}

function workspaceEdit(monaco: Monaco, edit?: LspWorkspaceEdit) {
  const edits: Array<{
    resource: ReturnType<Monaco["Uri"]["parse"]>;
    textEdit: { range: ReturnType<typeof range>; text: string };
    versionId: number | undefined;
  }> = [];
  for (const [uri, textEdits] of Object.entries(edit?.changes ?? {})) {
    for (const textEdit of textEdits) {
      edits.push({
        resource: monaco.Uri.parse(uri),
        textEdit: {
          range: range(monaco, textEdit.range),
          text: textEdit.newText,
        },
        versionId: undefined,
      });
    }
  }
  return { edits };
}

function completionKind(monaco: Monaco, kind?: number) {
  const values = [
    monaco.languages.CompletionItemKind.Text,
    monaco.languages.CompletionItemKind.Method,
    monaco.languages.CompletionItemKind.Function,
    monaco.languages.CompletionItemKind.Constructor,
    monaco.languages.CompletionItemKind.Field,
    monaco.languages.CompletionItemKind.Variable,
    monaco.languages.CompletionItemKind.Class,
    monaco.languages.CompletionItemKind.Interface,
    monaco.languages.CompletionItemKind.Module,
    monaco.languages.CompletionItemKind.Property,
    monaco.languages.CompletionItemKind.Unit,
    monaco.languages.CompletionItemKind.Value,
    monaco.languages.CompletionItemKind.Enum,
    monaco.languages.CompletionItemKind.Keyword,
    monaco.languages.CompletionItemKind.Snippet,
    monaco.languages.CompletionItemKind.Color,
    monaco.languages.CompletionItemKind.File,
    monaco.languages.CompletionItemKind.Reference,
    monaco.languages.CompletionItemKind.Folder,
    monaco.languages.CompletionItemKind.EnumMember,
    monaco.languages.CompletionItemKind.Constant,
    monaco.languages.CompletionItemKind.Struct,
    monaco.languages.CompletionItemKind.Event,
    monaco.languages.CompletionItemKind.Operator,
    monaco.languages.CompletionItemKind.TypeParameter,
  ];
  return values[Math.max(0, (kind ?? 1) - 1)] ?? monaco.languages.CompletionItemKind.Text;
}

function symbolKind(monaco: Monaco, kind: number) {
  const values = [
    monaco.languages.SymbolKind.File,
    monaco.languages.SymbolKind.Module,
    monaco.languages.SymbolKind.Namespace,
    monaco.languages.SymbolKind.Package,
    monaco.languages.SymbolKind.Class,
    monaco.languages.SymbolKind.Method,
    monaco.languages.SymbolKind.Property,
    monaco.languages.SymbolKind.Field,
    monaco.languages.SymbolKind.Constructor,
    monaco.languages.SymbolKind.Enum,
    monaco.languages.SymbolKind.Interface,
    monaco.languages.SymbolKind.Function,
    monaco.languages.SymbolKind.Variable,
    monaco.languages.SymbolKind.Constant,
    monaco.languages.SymbolKind.String,
    monaco.languages.SymbolKind.Number,
    monaco.languages.SymbolKind.Boolean,
    monaco.languages.SymbolKind.Array,
    monaco.languages.SymbolKind.Object,
    monaco.languages.SymbolKind.Key,
    monaco.languages.SymbolKind.Null,
    monaco.languages.SymbolKind.EnumMember,
    monaco.languages.SymbolKind.Struct,
    monaco.languages.SymbolKind.Event,
    monaco.languages.SymbolKind.Operator,
    monaco.languages.SymbolKind.TypeParameter,
  ];
  return values[Math.max(0, kind - 1)] ?? monaco.languages.SymbolKind.Object;
}

function location(monaco: Monaco, value: LspLocation | LspLocationLink) {
  if ("targetUri" in value) {
    return {
      uri: monaco.Uri.parse(value.targetUri),
      range: range(monaco, value.targetSelectionRange ?? value.targetRange),
    };
  }
  return {
    uri: monaco.Uri.parse(value.uri),
    range: range(monaco, value.range),
  };
}

function position(model: { uri: { toString(): string } }, line: number, column: number) {
  return {
    textDocument: { uri: model.uri.toString() },
    position: { line: line - 1, character: column - 1 },
  };
}

export function registerLspProviders(
  monaco: Monaco,
  client: OpenForgeClient,
  language: string,
  sessionId: string,
  initializeResult: unknown,
) {
  const disposables: Array<{ dispose(): void }> = [];
  const root = record(initializeResult);
  const server = record(root?.capabilities) ?? {};

  if (server.completionProvider) {
    const completion = record(server.completionProvider);
    const triggerCharacters = Array.isArray(completion?.triggerCharacters)
      ? completion.triggerCharacters.filter(
          (value): value is string => typeof value === "string",
        )
      : [];
    disposables.push(
      monaco.languages.registerCompletionItemProvider(language, {
        triggerCharacters,
        async provideCompletionItems(model, cursor) {
          const response = await client.lspRequest<
            LspCompletionItem[] | { items?: LspCompletionItem[] } | null
          >(
            sessionId,
            "textDocument/completion",
            position(model, cursor.lineNumber, cursor.column),
          );
          const items = Array.isArray(response) ? response : response?.items ?? [];
          const word = model.getWordUntilPosition(cursor);
          return {
            suggestions: items.map((item) => {
              const suggestion: {
                label: string;
                kind: number;
                insertText: string;
                range: ReturnType<typeof range>;
                detail?: string;
                documentation?: string;
                insertTextRules?: number;
              } = {
                label: item.label,
                kind: completionKind(monaco, item.kind),
                insertText: item.textEdit?.newText ?? item.insertText ?? item.label,
                range: item.textEdit
                  ? range(monaco, item.textEdit.range)
                  : new monaco.Range(
                      cursor.lineNumber,
                      word.startColumn,
                      cursor.lineNumber,
                      word.endColumn,
                    ),
              };
              if (item.detail) suggestion.detail = item.detail;
              const docs = documentation(item.documentation);
              if (docs) suggestion.documentation = docs;
              if (item.insertTextFormat === 2) {
                suggestion.insertTextRules =
                  monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet;
              }
              return suggestion;
            }),
          };
        },
      }),
    );
  }

  if (server.hoverProvider) {
    disposables.push(
      monaco.languages.registerHoverProvider(language, {
        async provideHover(model, cursor) {
          const response = await client.lspRequest<LspHover | null>(
            sessionId,
            "textDocument/hover",
            position(model, cursor.lineNumber, cursor.column),
          );
          if (!response) return null;
          const result: {
            contents: Array<{ value: string }>;
            range?: ReturnType<typeof range>;
          } = { contents: [{ value: hoverContents(response.contents) }] };
          if (response.range) result.range = range(monaco, response.range);
          return result;
        },
      }),
    );
  }

  if (server.definitionProvider) {
    disposables.push(
      monaco.languages.registerDefinitionProvider(language, {
        async provideDefinition(model, cursor) {
          const response = await client.lspRequest<
            LspLocation | LspLocation[] | LspLocationLink | LspLocationLink[] | null
          >(
            sessionId,
            "textDocument/definition",
            position(model, cursor.lineNumber, cursor.column),
          );
          if (!response) return [];
          return (Array.isArray(response) ? response : [response]).map((entry) =>
            location(monaco, entry),
          );
        },
      }),
    );
  }

  if (server.referencesProvider) {
    disposables.push(
      monaco.languages.registerReferenceProvider(language, {
        async provideReferences(model, cursor, context) {
          const response = await client.lspRequest<LspLocation[] | null>(
            sessionId,
            "textDocument/references",
            {
              ...position(model, cursor.lineNumber, cursor.column),
              context: { includeDeclaration: context.includeDeclaration },
            },
          );
          return (response ?? []).map((entry) => location(monaco, entry));
        },
      }),
    );
  }

  if (server.renameProvider) {
    disposables.push(
      monaco.languages.registerRenameProvider(language, {
        async provideRenameEdits(model, cursor, newName) {
          const response = await client.lspRequest<LspWorkspaceEdit>(
            sessionId,
            "textDocument/rename",
            {
              ...position(model, cursor.lineNumber, cursor.column),
              newName,
            },
          );
          return workspaceEdit(monaco, response);
        },
        async resolveRenameLocation(model, cursor) {
          const response = await client.lspRequest<
            LspRange | { range: LspRange; placeholder: string } | null
          >(
            sessionId,
            "textDocument/prepareRename",
            position(model, cursor.lineNumber, cursor.column),
          );
          if (!response) return null;
          if ("start" in response) {
            return { range: range(monaco, response), text: model.getValueInRange(range(monaco, response)) };
          }
          return {
            range: range(monaco, response.range),
            text: response.placeholder,
          };
        },
      }),
    );
  }

  if (server.codeActionProvider) {
    disposables.push(
      monaco.languages.registerCodeActionProvider(language, {
        async provideCodeActions(model, selection, context) {
          const actions = await client.lspRequest<LspCodeAction[] | null>(
            sessionId,
            "textDocument/codeAction",
            {
              textDocument: { uri: model.uri.toString() },
              range: {
                start: {
                  line: selection.startLineNumber - 1,
                  character: selection.startColumn - 1,
                },
                end: {
                  line: selection.endLineNumber - 1,
                  character: selection.endColumn - 1,
                },
              },
              context: {
                diagnostics: context.markers.map((marker) => ({
                  range: {
                    start: {
                      line: marker.startLineNumber - 1,
                      character: marker.startColumn - 1,
                    },
                    end: {
                      line: marker.endLineNumber - 1,
                      character: marker.endColumn - 1,
                    },
                  },
                  severity: marker.severity,
                  message: marker.message,
                  source: marker.source,
                  code: marker.code,
                })),
                only: context.only,
              },
            },
          );
          return {
            actions: (actions ?? [])
              .filter((action) => action.edit)
              .map((action) => ({
                title: action.title,
                ...(action.kind !== undefined ? { kind: action.kind } : {}),
                ...(action.isPreferred !== undefined
                  ? { isPreferred: action.isPreferred }
                  : {}),
                edit: workspaceEdit(monaco, action.edit),
              })),
            dispose() {},
          };
        },
      }),
    );
  }

  if (server.signatureHelpProvider) {
    const signature = record(server.signatureHelpProvider);
    const triggerCharacters = Array.isArray(signature?.triggerCharacters)
      ? signature.triggerCharacters.filter(
          (value): value is string => typeof value === "string",
        )
      : ["(", ","];
    disposables.push(
      monaco.languages.registerSignatureHelpProvider(language, {
        signatureHelpTriggerCharacters: triggerCharacters,
        async provideSignatureHelp(model, cursor) {
          const response = await client.lspRequest<LspSignatureHelp | null>(
            sessionId,
            "textDocument/signatureHelp",
            position(model, cursor.lineNumber, cursor.column),
          );
          if (!response) return null;
          return {
            value: {
              signatures: response.signatures.map((signatureItem) => ({
                label: signatureItem.label,
                documentation: documentation(signatureItem.documentation),
                parameters: (signatureItem.parameters ?? []).map((parameter) => ({
                  label: parameter.label,
                  documentation: documentation(parameter.documentation),
                })),
              })),
              activeSignature: response.activeSignature ?? 0,
              activeParameter: response.activeParameter ?? 0,
            },
            dispose() {},
          };
        },
      }),
    );
  }

  if (server.documentSymbolProvider) {
    disposables.push(
      monaco.languages.registerDocumentSymbolProvider(language, {
        async provideDocumentSymbols(model) {
          const symbols = await client.lspRequest<LspDocumentSymbol[] | null>(
            sessionId,
            "textDocument/documentSymbol",
            { textDocument: { uri: model.uri.toString() } },
          );
          const convert = (symbol: LspDocumentSymbol): unknown => ({
            name: symbol.name,
            detail: symbol.detail ?? "",
            kind: symbolKind(monaco, symbol.kind),
            range: range(monaco, symbol.range),
            selectionRange: range(monaco, symbol.selectionRange),
            tags: [],
            children: (symbol.children ?? []).map(convert),
          });
          return (symbols ?? []).map(convert) as never[];
        },
      }),
    );
  }

  const semantic = record(server.semanticTokensProvider);
  const legend = record(semantic?.legend);
  const tokenTypes = Array.isArray(legend?.tokenTypes)
    ? legend.tokenTypes.filter((value): value is string => typeof value === "string")
    : [];
  const tokenModifiers = Array.isArray(legend?.tokenModifiers)
    ? legend.tokenModifiers.filter(
        (value): value is string => typeof value === "string",
      )
    : [];
  if (tokenTypes.length) {
    disposables.push(
      monaco.languages.registerDocumentSemanticTokensProvider(language, {
        getLegend: () => ({ tokenTypes, tokenModifiers }),
        async provideDocumentSemanticTokens(model) {
          const result = await client.lspRequest<{ data?: number[] } | null>(
            sessionId,
            "textDocument/semanticTokens/full",
            { textDocument: { uri: model.uri.toString() } },
          );
          return { data: new Uint32Array(result?.data ?? []) };
        },
        releaseDocumentSemanticTokens() {},
      }),
    );
  }

  return {
    dispose() {
      for (const disposable of disposables) disposable.dispose();
    },
  };
}
