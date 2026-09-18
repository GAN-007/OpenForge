# ADR 0001: Daemon-first control plane

Status: Accepted

## Decision

OpenForge has one canonical daemon/runtime. Desktop, VS Code, JetBrains, web, CLI, SDKs and external agents are clients or workers rather than independent implementations of the engineering state machine.

## Consequences

Run/task/audit semantics remain consistent across surfaces. Clients can be replaced without migrating project state. The daemon becomes a critical security and compatibility boundary and therefore requires explicit versioning and hardened transports.
