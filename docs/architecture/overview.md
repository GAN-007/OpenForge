# Architecture Overview

OpenForge is a daemon-first AI engineering control plane. Every client is replaceable; the daemon owns canonical execution state.

## Trust and execution flow

```text
User / IDE / CLI / API
          |
          v
   OpenForge daemon
          |
   +------+-------+------------------+
   |              |                  |
 policy        scheduler         model fabric
   |              |                  |
   v              v                  v
 brokers      task DAG        provider adapters
   |
   +--> Git workspace broker
   +--> sandbox backend
   +--> MCP / ACP
   +--> browser worker
   +--> future DB/cloud brokers
```

The model never receives implicit host authority. A generated action is first decoded into a structured operation, then evaluated by policy, then executed through a broker or sandbox.

## Canonical state

SQLite WAL is the local state store. Runs, tasks, events, memory and costs are durable. The event stream is hash-linked to provide tamper evidence. Team deployments can replace local persistence behind the store boundary without changing client contracts.

## Multi-agent execution

A planner produces a finite DAG. The scheduler validates dependency references and cycle freedom. Independent ready nodes may execute concurrently.

Each task gets an `of/task/<task-id>` branch and worktree. The run also owns `of/integration/<run-id>`. Agents never write directly to the user's active branch.

Successful task flow:

```text
agent loop
  -> formatter/linter/test acceptance
  -> task commit
  -> integration cherry-pick
  -> acceptance rerun on combined state
  -> task complete
```

Conflicts fail the affected integration step instead of silently overwriting another agent.

## Isolation

Worktrees provide edit concurrency, not malicious-code isolation. `autonomous` mode is rejected without an isolated sandbox backend. Docker is built in; the runner interface is designed for Podman, DevContainers, remote VMs, microVMs and Kubernetes.

## Model fabric

The fabric flattens provider catalogues into capability-aware model specifications. The router rejects models that violate context, tool, vision, structured-output, privacy, latency or hard-cost constraints before scoring quality, privacy, cost and latency.

Agent, completion and review requests can therefore be routed independently without moving canonical task state into a provider.

## Client surfaces

The TypeScript and Python SDKs speak JSON-RPC to the daemon. VS Code, web and Tauri clients consume the same API. JetBrains uses its own native client. ACP is available for editors/external coding agents; MCP is available for tools and data systems.
