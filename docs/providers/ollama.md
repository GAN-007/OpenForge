# Ollama provider

OpenForge has two Ollama-compatible paths:

1. **Native Ollama provider** (`kind: ollama`) for local runtime controls such as `keep_alive`, `num_ctx`, `num_gpu`, installed-model checks and RAM preflight.
2. **Generic OpenAI-compatible provider** (`kind: openai-compatible`) for Ollama's `/v1` compatibility endpoint and other compatible gateways.

The checked-in `openforge.yaml` uses the native path and registers:

- `qwen2.5-coder:7b`
- `deepseek-coder:6.7b`
- `deepseek-coder-v2:16b`
- `deepseek-r1:7b`

Only models explicitly marked with `tools: true` can satisfy a tool-required routing request. Local models are priced at zero in the default configuration, so routing primarily differentiates them by capability, quality, privacy and latency.

## Preflight

Before planning, the daemon asks every configured provider for a readiness report. For local Ollama this checks the loopback endpoint, verifies that the configured model exists, detects whether it is already loaded, reads Linux `MemAvailable` when the endpoint is local, and estimates load headroom from the installed model size. Missing or blocked candidates are skipped by the fabric fallback chain.

Inspect readiness from any client:

```bash
curl -s http://127.0.0.1:8765/v1/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"model/preflight","params":{}}' | jq .
```

The web and desktop interfaces show the same status and can explicitly pull missing models through the policy-gated local Ollama MCP plugin.

## Verification

Run the full local verification from the repository root:

```bash
./verify-ollama-openforge.sh
```

It verifies prerequisites, configured-vs-installed models, Rust formatting/check/tests, daemon startup, model catalog/preflight, a real OpenForge completion request, and direct native Ollama chat calls for installed configured models.

Large models can remain configured even when the current machine cannot load them. The preflight/fallback path prevents an oversized preferred model from turning a run into a long opaque failure; install or remove models according to the target machine's RAM.
