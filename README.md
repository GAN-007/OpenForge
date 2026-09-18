# OpenForge

OpenForge is an open-source AI engineering operating system: a protocol-first control plane for secure, auditable, multi-agent software engineering across CLI, desktop, IDE, web, API, local and remote execution environments.

## Architecture

OpenForge is built around a single canonical daemon and event model. All user surfaces and external agents connect to that control plane rather than implementing independent agent runtimes.

The core owns:

- durable runs, task DAGs and agent leases;
- policy decisions and approval flows;
- Git workspaces and integration branches;
- sandbox lifecycle and privileged brokers;
- provider-neutral model routing;
- repository/context intelligence;
- MCP and ACP interoperability;
- immutable, attributable audit events;
- budget enforcement and cost accounting.

Autonomous execution is never treated as equivalent to host execution. OpenForge requires a sandbox backend for autonomous mode and keeps reusable provider credentials outside code-execution sandboxes.

## Repository status

This repository is the canonical implementation of OpenForge. The codebase is organized as a multi-language monorepo with a Rust control plane, TypeScript clients and SDKs, a Tauri desktop application, a VS Code extension, a Kotlin JetBrains client, Python integrations/evaluations, shared schemas, runners, plugins and end-to-end tests.

## License

Apache-2.0. See `LICENSE` and `NOTICE`.
