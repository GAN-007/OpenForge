# Sevi gateway in OpenForge

Use your authorized Sevi gateway account directly. Cursor and a Cursor subscription are not required. OpenForge is open-source; that does not determine Sevi's upstream charges, allowances or quotas.

## GUI setup

1. Start the OpenForge daemon and either the web console or desktop frontend.
2. Find **Model connection → Sevi model gateway**.
3. Paste your actual key into **Sevi API key** (not the separate OpenForge daemon access-token field).
4. Click **Test & connect**. This sends one short completion test containing no repository data. Plain text and fenced replies are accepted; connection verification does not require JSON.
5. On success, new planning, agent tasks and completion requests use Sevi. Create/select a run and plan its objective as usual.

Preset:

- Base URL: `https://model.sevi.io/cursor`
- Model: `auto-select`
- Transport: OpenAI-compatible `POST /cursor/chat/completions` with Bearer authorization. No extra `/v1` is appended.

After a successful test, the daemon saves the key in `$XDG_CONFIG_HOME/openforge/sevi-gateway.key`, or `~/.config/openforge/sevi-gateway.key` when XDG_CONFIG_HOME is unset. The Unix directory has mode 0700 and the file mode 0600; writes are atomic and readers reject symlinks, wrong ownership and permissive file modes. This file is plaintext protected by OS permissions, not an encrypted keychain. It is outside the repository and never written to project YAML, SQLite, browser storage or API responses. Persistence currently requires Unix permissions (Linux/macOS).

The daemon restores the saved provider at startup without an extra billable test. The password field clears on success and stays empty after reload; **Connected · key remembered** confirms persistence. **Disconnect & forget key** deletes the saved credential and restores the original provider for new operations. In-flight tasks can still finish. Invalid replacement keys do not overwrite the saved key or active provider. The shared CLI and GUI use the same daemon; explicit CLI `--standalone` mode still uses its separate configured engine.

Sevi controls authentication, available models and quotas. The test reports failed authentication, HTTP errors, timeout or missing assistant text without returning upstream diagnostic bodies that could expose credentials. The adapter redacts credentials in debug formatting.

## Cost and routing limits

No price is assumed for the `auto-select` alias. If LiteLLM supplies `x-litellm-response-cost`, OpenForge records it for normal run calls; otherwise the preset has no local USD estimate. A displayed zero is not proof that a request is free. The setup test is outside a run and is not added to a run budget. Gateway-side limits remain authoritative; unknown pricing cannot enforce a reliable local dollar ceiling.

The preset uses 32,768 context tokens and a latency score suitable for selection by the current planning/completion router. These are client routing defaults, not guarantees about the model dynamically selected by Sevi. The connection probe verifies successful completion with nonempty assistant text. It does not establish that every auto-selected model supports planner JSON; planning and agent actions still validate their own structured output. Larger-context, vision and native tool-call support are not inferred from the alias.

## Local startup

```bash
cargo run -p openforge-daemon -- --config openforge.yaml
pnpm build
pnpm dev:web
```

The default daemon port is 8765 and web development port is 5173. If other applications already occupy them, select explicit alternatives:

```bash
OPENFORGE_ALLOWED_ORIGINS=http://127.0.0.1:5180,http://localhost:5180 \
  cargo run -p openforge-daemon -- --config openforge.yaml --listen 127.0.0.1:8875

VITE_OPENFORGE_DAEMON_URL=http://127.0.0.1:8875 \
  pnpm --filter @openforge/web-console dev --host 127.0.0.1 --port 5180 --strictPort
```

`VITE_OPENFORGE_DAEMON_URL` is a public frontend configuration value, not a place for credentials. Remote daemons require authenticated TLS transport; do not send provider keys across untrusted plaintext networks.

## Settings API

All methods inherit the daemon's existing authorization:

- `gateway/status` returns connected state, URL, model, storage mode, `credential_persisted` and pricing mode, never the key.
- `gateway/connect` accepts `{api_key}` and tests the fixed Sevi endpoint before switching the model provider. A failed test leaves the current provider active.
- `gateway/disconnect` deletes the saved key, removes the override and restores the configured model fabric.

The TypeScript SDK exposes `gatewayStatus`, `connectGateway`, `disconnectGateway`; Python uses `gateway_status`, `connect_gateway`, `disconnect_gateway`. The capability flag is `gateway_settings`.

Tests use a mock gateway to verify the exact path/model/Bearer contract, plain/fenced setup responses, private-file permissions and restart restoration, cost headers, real engine completion routing, restoration and secret-safe errors. The browser test verifies masked input, clearing and absence from browser storage. A valid Sevi key must be supplied by the user to verify their live account.
