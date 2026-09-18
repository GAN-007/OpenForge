# OpenForge JSON-RPC Protocol

Protocol identifier: `openforge.protocol.v1`.

The local daemon accepts JSON-RPC 2.0 requests at `POST /v1/rpc`. Request IDs are client-generated. Errors use standard JSON-RPC-shaped error objects with OpenForge application codes in the `-32000` range.

## Initialization

`initialize` returns protocol/server versions and a capability map. Clients must not assume an optional capability exists without checking it.

## Implemented methods

| Method | Purpose |
| --- | --- |
| `initialize` | negotiate server capabilities |
| `run/create` | pin objective and repository base SHA |
| `run/plan` | create and validate a task DAG |
| `run/execute` | execute a planned run |
| `run/get` | read durable run state |
| `task/list` | list task nodes for a run |
| `event/list` | replay ordered audit events |
| `budget/get` | read run spend |
| `memory/put` | write explicit scoped memory |
| `memory/search` | search explicit memory |
| `memory/delete` | delete explicit memory |
| `completion/request` | latency-oriented insertion request |
| `model/providers` | list configured provider names |

## Transport boundaries

OpenForge JSON-RPC is the product control API. It does not replace MCP or ACP.

- MCP is used for agent/tool and data integrations.
- ACP is used for editor/external-agent interoperability.
- LSP remains the language-intelligence protocol.
- DAP remains the debugger protocol.

These protocols can evolve independently while the OpenForge canonical state remains stable.

## Compatibility

Breaking schema or method changes require a new protocol version. Additive fields must be ignored by older clients unless a negotiated capability says otherwise.
