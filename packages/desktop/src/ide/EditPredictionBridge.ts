import type { Monaco } from "@monaco-editor/react";
import type {
  DiagnosticContext,
  OpenForgeClient,
  RecentEdit,
} from "@openforge/sdk";

export function registerEditPredictionProvider(
  monaco: Monaco,
  client: OpenForgeClient,
  language: string,
  options: {
    getRunId: () => string;
    getPath: (uri: string) => string | undefined;
    getRecentEdits: () => RecentEdit[];
    getDiagnostics: (path: string) => DiagnosticContext[];
    getSemanticContext: (path: string) => string;
  },
) {
  return monaco.languages.registerInlineCompletionsProvider(language, {
    async provideInlineCompletions(model, position, _context, token) {
      const runId = options.getRunId();
      const path = options.getPath(model.uri.toString());
      if (!runId || !path) {
        return { items: [], dispose() {} };
      }

      const offset = model.getOffsetAt(position);
      const value = model.getValue();
      const prefix = value.slice(0, offset);
      const suffix = value.slice(offset);
      const controller = new AbortController();
      const disposable = token.onCancellationRequested(() => controller.abort());

      try {
        const prediction = await client.predictEdits(
          {
            run_id: runId,
            file_path: path,
            language,
            prefix,
            suffix,
            recent_edits: options.getRecentEdits().slice(0, 12),
            diagnostics: options.getDiagnostics(path).slice(0, 50),
            semantic_context: options.getSemanticContext(path),
            max_cost_usd: 0.03,
            max_latency_ms: 1500,
          },
          controller.signal,
        );
        if (token.isCancellationRequested || prediction.confidence < 0.35) {
          return { items: [], dispose() {} };
        }

        const items = prediction.edits
          .filter((edit) => edit.file_path === path)
          .map((edit) => {
            return {
              insertText: edit.new_text,
              range: new monaco.Range(
                edit.range.start_line + 1,
                edit.range.start_column + 1,
                edit.range.end_line + 1,
                edit.range.end_column + 1,
              ),
            };
          });

        return { items, dispose() {} };
      } catch (error) {
        if (!controller.signal.aborted) {
          console.debug("OpenForge edit prediction skipped", error);
        }
        return { items: [], dispose() {} };
      } finally {
        disposable.dispose();
      }
    },
    freeInlineCompletions() {},
  });
}
