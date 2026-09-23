# Ollama local models

OpenForge keeps Ollama on the existing `openai-compatible` provider contract so vLLM, LM Studio, hosted gateways, and other compatible endpoints continue to use the same generic adapter. For a local `http://127.0.0.1:11434/v1`, `localhost:11434/v1`, or `host.docker.internal:11434/v1` endpoint, the adapter automatically activates the native Ollama `/api/chat` path before falling back to `/v1/chat/completions`.

The native path adds the controls the compatibility shim cannot carry: model residency through `keep_alive`, request context through `num_ctx`, optional GPU allocation through `num_gpu`, and a preflight that checks `/api/tags` plus available Linux memory before loading a model. If the chosen model is absent or does not fit, the normal model fabric can continue to another eligible configured model.

## Checked-in local catalog

`openforge.yaml` registers:

- `qwen2.5-coder:7b` — tools and structured output enabled;
- `deepseek-coder:6.7b` — structured output, no tool routing;
- `deepseek-coder-v2:16b` — structured output, no tool routing;
- `deepseek-r1:7b` — reasoning model, no tools and no structured-output routing.

Pull only the models you intend to run:

```bash
ollama pull qwen2.5-coder:7b
ollama pull deepseek-coder:6.7b
ollama pull deepseek-r1:7b
# Pull deepseek-coder-v2:16b only when the host has enough free memory.
```

The daemon does not require every configured model to be installed at startup. Availability is checked immediately before a native Ollama invocation, so a missing or oversized preferred model can fail cleanly and allow fabric fallback.

## Native runtime controls

Defaults are conservative and can be overridden for the daemon process:

```bash
export OPENFORGE_OLLAMA_KEEP_ALIVE=30m
export OPENFORGE_OLLAMA_NUM_CTX=32768
export OPENFORGE_OLLAMA_NUM_GPU=0
export OPENFORGE_OLLAMA_MEMORY_HEADROOM_MB=512
```

`OPENFORGE_OLLAMA_NATIVE=off` disables native detection and forces the generic OpenAI-compatible shim. `OPENFORGE_OLLAMA_NATIVE=on` forces native mode for a `/v1` endpoint when an advanced local deployment does not use the default host name. Credentials are never added by native detection.

## Model management

The built-in `dev.openforge.ollama` MCP plugin exposes four policy-gated tools: `list_models`, `show_model`, `preflight_model`, and `pull_model`. The default development policy allows those tools only through the named `ollama` MCP server configured in `openforge.yaml`.

The web console uses the same MCP boundary to show installed models, current available memory, per-model fit checks, and explicit pull actions. This keeps model downloads out of the daemon's canonical run API and inside the existing capability-policy boundary.

## Verification

Run the repository verification script from the checkout:

```bash
chmod +x verify-ollama-openforge.sh
./verify-ollama-openforge.sh
```

It checks local prerequisites, cross-checks configured model IDs against `ollama list`, builds the workspace, starts an isolated daemon state, verifies `model/list`, exercises the Ollama MCP inventory, and invokes one selected local model through `FabricProvider` using the native adapter.

For Docker access, see `runners/docker/ollama-bridge.md`.
