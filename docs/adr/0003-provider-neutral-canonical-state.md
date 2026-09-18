# ADR 0003: Provider-neutral canonical state

Status: Accepted

## Decision

Provider conversation IDs, hosted agent sessions and vendor traces are optional metadata. OpenForge owns canonical run, task, event, cost and memory state.

## Consequences

A provider can be replaced mid-project, local/offline execution remains possible, and audit semantics do not depend on vendor retention behavior.
