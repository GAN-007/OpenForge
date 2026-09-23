import type { OpenForgeClient } from "./client.js";

export interface OllamaModelInfo {
  name: string;
  model: string;
  size: number;
  digest: string;
  modified_at: string;
  details: Record<string, unknown>;
}

export interface OllamaInventory {
  models: OllamaModelInfo[];
  available_memory_bytes: number | null;
}

export interface OllamaPreflight {
  ready: boolean;
  installed: boolean;
  model: string;
  context_tokens: number;
  model_size_bytes?: number;
  available_memory_bytes: number | null;
  estimated_required_memory_bytes: number | null;
  message: string;
}

interface McpResult {
  content: unknown[];
  isError: boolean;
  structuredContent?: unknown;
}

function decodeMcp<T>(result: McpResult): T {
  if (result.isError) {
    const message = result.content
      .filter((item): item is { type: string; text: string } =>
        typeof item === "object" && item !== null &&
        "type" in item && (item as { type?: unknown }).type === "text" &&
        "text" in item && typeof (item as { text?: unknown }).text === "string")
      .map((item) => item.text)
      .join("\n");
    throw new Error(message || "Ollama MCP call failed");
  }
  if (result.structuredContent !== undefined) {
    return result.structuredContent as T;
  }
  for (const item of result.content) {
    if (
      typeof item === "object" && item !== null &&
      "type" in item && (item as { type?: unknown }).type === "text" &&
      "text" in item && typeof (item as { text?: unknown }).text === "string"
    ) {
      return JSON.parse((item as { text: string }).text) as T;
    }
  }
  throw new Error("Ollama MCP response contained no structured result");
}

export async function listOllamaModels(client: OpenForgeClient): Promise<OllamaInventory> {
  return decodeMcp<OllamaInventory>(
    await client.mcpCallTool("ollama", "list_models", {}),
  );
}

export async function preflightOllamaModel(
  client: OpenForgeClient,
  model: string,
  contextTokens: number,
): Promise<OllamaPreflight> {
  return decodeMcp<OllamaPreflight>(
    await client.mcpCallTool("ollama", "preflight_model", {
      model,
      context_tokens: contextTokens,
    }),
  );
}

export async function showOllamaModel(
  client: OpenForgeClient,
  model: string,
): Promise<Record<string, unknown>> {
  return decodeMcp<Record<string, unknown>>(
    await client.mcpCallTool("ollama", "show_model", { model }),
  );
}

export async function pullOllamaModel(
  client: OpenForgeClient,
  model: string,
): Promise<Record<string, unknown>> {
  return decodeMcp<Record<string, unknown>>(
    await client.mcpCallTool("ollama", "pull_model", { model }),
  );
}
