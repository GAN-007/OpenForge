# OpenForge

OpenForge is an open-source AI engineering operating system: a protocol-first control plane for secure, auditable, multi-agent software engineering across CLI, desktop, IDE, web, API, local and remote execution environments.

It is not an editor fork and it does not combine six upstream agent repositories into a monolith. The canonical state lives in one daemon: runs, task DAGs, policy decisions, model usage, Git integration, memory and the append-only audit stream are shared by every client.

## What is implemented

The repository contains a working Rust control plane and CLI, SQLite WAL event/state store, hash-linked audit ledger, deterministic DAG scheduler, concurrent task worktrees, integration-branch merge coordinator, policy engine, Docker/local execution backends, model fabric, cost accounting, explicit memory, MCP/ACP process clients, repository indexing, browser worker, TypeScript/Python SDKs, web console, Tauri desktop shell, VS Code extension with inline completion, and JetBrains client.

Model providers currently supported by the built-in fabric are:

- OpenAI-compatible HTTP endpoints, including Ollama, vLLM, LM Studio, OpenRouter-compatible gateways and compatible hosted APIs;
- Anthropic Messages API;
- Google Gemini generateContent API;
- AWS Bedrock Converse through the authenticated AWS CLI.

The provider abstraction is intentionally independent of the canonical OpenForge run/event state.

## Security model

OpenForge separates authorization from containment. Agent policy decides whether an action is allowed, denied or requires approval. Autonomous mode additionally requires an isolated runner. A Git worktree is used for concurrency and review; it is never treated as a malicious-code sandbox.

The daemon binds to `127.0.0.1:8765` by default. Browser CORS is restricted to the local development and Tauri origins compiled into the daemon. Do not expose the local daemon directly to an untrusted network; place remote/team deployments behind authenticated TLS ingress.

The default development policy denies common secret paths, host Git metadata, privilege escalation, SSH, pushes, destructive cluster commands and production-like secret access.

## Quick start

Requirements:

- Rust 1.88 or newer;
- Git;
- one configured model provider;
- Docker when using autonomous mode;
- Node 22 and pnpm 10 for the web/desktop/VS Code surfaces;
- Python 3.10+ for the Python SDK.

Build and test the Rust workspace:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
```

Initialize OpenForge in an existing Git repository:

```bash
cargo run -p openforge -- init /absolute/path/to/repository
```

The checked-in `openforge.yaml` is local-first and points at an OpenAI-compatible Ollama endpoint. Configure the model ID and endpoint to match a model that actually exists in your environment.

Run an engineering objective:

```bash
cargo run -p openforge -- run "Fix the failing tests and verify the application" /absolute/path/to/repository --budget 10 --mode execute
```

Autonomous execution requires the Docker runner:

```bash
docker build -t ghcr.io/gan-007/openforge-runner:latest runners/docker
cargo run -p openforge -- run "Implement and verify the requested change" /absolute/path/to/repository --mode autonomous --docker
```

Start the control-plane API:

```bash
cargo run -p openforge-daemon -- --config openforge.yaml --listen 127.0.0.1:8765
```

Build the TypeScript clients:

```bash
corepack enable
pnpm install
pnpm typecheck
pnpm build
```

Build and test the Python SDK:

```bash
python -m pip install -e "python[dev]"
pytest python/tests
```

## Engineering flow

A run is pinned to an immutable base SHA. The planner produces a validated DAG. Runnable independent tasks start from the same accepted integration SHA and execute in separate task worktrees. Successful tasks must satisfy their acceptance commands before they can produce a commit. The merge coordinator cherry-picks accepted task commits into `of/integration/<run-id>`, reruns the relevant acceptance checks against the combined state, and records every step in the event ledger.

The user's original branch is not implicitly modified.

## Memory

Memory is explicit and inspectable. Supported scopes are `session`, `project`, `repository`, `user_rules`, and `agent_experience`.

```bash
openforge memory put --scope project --key architecture --value-json '{"decision":"daemon-first"}'
openforge memory search daemon --scope project
openforge memory forget architecture --scope project
```

## Repository layout

- `crates/`: Rust control plane, execution, routing, policy, protocol, MCP/ACP and CLI/daemon crates.
- `packages/`: TypeScript SDK, UI, web console, desktop frontend, browser worker and VS Code extension.
- `python/`: Python SDK and tests.
- `jetbrains/`: Kotlin IntelliJ-platform client.
- `schemas/`: stable protocol/event/task/plugin contracts.
- `plugins/`: built-in capability-manifested plugins.
- `runners/`: Docker, DevContainer and Kubernetes execution definitions.
- `docs/`: architecture, protocol, security, provider and contribution documentation.
- `evals/`: regression suites for routing, security, repository understanding and multi-agent execution.
- `tests/`: cross-component integration and protocol tests.

## Protocol

The public protocol version is `openforge.protocol.v2`. The daemon exposes JSON-RPC 2.0 at `POST /v1/rpc`, a health endpoint at `GET /health`, authenticated REST adapters, and resumable audit streams at `GET /v1/runs/{id}/events/stream`. See [API documentation](docs/protocols/json-rpc.md). MCP and ACP are implemented as separate interoperability boundaries rather than being confused with the OpenForge canonical API.

See the [repository audit and roadmap](docs/audit/2026-09-20.md) for verified fixes, remaining gaps, the file inventory, and branch/PR analysis.

## License

OpenForge is licensed under Apache-2.0. See `LICENSE`, `NOTICE`, and `THIRD_PARTY_LICENSES/README.md`.
