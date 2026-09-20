# OpenForge control-plane API

Protocol identifier: `openforge.protocol.v2`. HTTP paths remain `/v1` for compatibility.

## Authorization and errors

`GET /health` is public. When `OPENFORGE_API_TOKEN` is set, RPC, REST and SSE require `Authorization: Bearer <token>`. The daemon is a trusted local control plane, not a multi-tenant service: one token grants broad authority. Bind locally or put remote deployments behind authenticated TLS ingress. Browser origins are controlled by `OPENFORGE_ALLOWED_ORIGINS`; tokens must not be placed in URLs.

JSON-RPC requests go to `POST /v1/rpc` with `jsonrpc`, `id`, `method`, and an object `params`. Response IDs mirror request IDs. Application errors use HTTP 400 and `error: {code, message}`; unauthorized calls use HTTP 401. The current implementation does not implement batches/notifications or every standard JSON-RPC error code. REST returns the underlying result without an RPC envelope, HTTP 401 for authentication failures, 404 for absent run resources and 400 for invalid operations. Extractor errors (such as malformed UUID/JSON) use Axum's transport error format.

## Initialization and RPC inventory

Call `initialize` to discover protocol/server versions and capabilities. Optional features should be negotiated. Methods use slashes, not dots. The following inventory is extracted from the daemon dispatch table:

| Domain | Implemented methods |
| --- | --- |
| initialize | `initialize` |
| run | `run/create`, `run/plan`, `run/execute`, `run/list`, `run/get` |
| task | `task/list` |
| event | `event/list`, `event/verify` |
| budget | `budget/get`, `budget/reserve`, `budget/settle`, `budget/snapshot` |
| memory | `memory/put`, `memory/search`, `memory/delete` |
| completion | `completion/request` |
| repository | `repository/index` |
| search | `search/query` |
| symbols | `symbols/query`, `symbols/graph` |
| artifact | `artifact/put`, `artifact/get`, `artifact/descriptor`, `artifact/delete`, `artifact/stream/begin`, `artifact/stream/chunk`, `artifact/stream/commit`, `artifact/stream/abort` |
| policy | `policy/evaluate` |
| telemetry | `telemetry/snapshot` |
| secret | `secret/lease`, `secret/revoke`, `secret/list` |
| acp | `acp/spawn`, `acp/request`, `acp/notify`, `acp/close`, `acp/list` |
| mcp | `mcp/list_tools`, `mcp/call_tool`, `mcp/list_resources`, `mcp/read_resource`, `mcp/list_prompts` |
| plugins | `plugins/list`, `plugins/capability/validate` |
| model | `model/list`, `model/providers` |

## REST adapters

These call the same control-plane handlers and store used by JSON-RPC.

| Method and path | Parameters / result |
| --- | --- |
| `GET /v1/runs` | `limit` (default 100, cap 1000), `offset` (default 0); newest-first run array, stable ID tie-break |
| `POST /v1/runs` | JSON `{repo, objective, budget_usd?, autonomy?}`; creates/pins a run, does not invoke a model |
| `GET /v1/runs/{id}` | Run or 404 |
| `GET /v1/runs/{id}/tasks` | Task array |
| `GET /v1/runs/{id}/events` | `after_sequence` (exclusive, default 0), `limit` (default 500, cap 5000) |
| `GET /v1/runs/{id}/events/stream` | Resumable SSE, below |
| `GET /v1/runs/{id}/budget` | Full budget snapshot, including persisted reservations |
| `GET /v1/models` | Configured model specifications; no provider credentials |
| `POST /v1/memory` | JSON `{scope, key, value, project_id?, repository_id?}` |
| `GET /v1/memory/search` | `query`, optional `scope`, `limit` (default 100, cap 1000) |
| `DELETE /v1/memory` | JSON `{key, scope?}`; current contract deletes matching keys across repository/project identifiers |
| `POST /v1/policy/evaluate` | JSON `{policy_path, capability, subject?, argv?}` |

Planning/execution and remaining integrations continue to use RPC. Durable cancellation, task retry/edit, artifact listing and multi-user secret administration are not implemented. Existing artifact put/get/delete and secret lease/revoke/list are RPC methods, not a generic unrestricted secret write API.

## Run workflow

1. `run/create`: repository path, objective, optional budget and autonomy. The repository must have a Git HEAD.
2. `run/plan`: `{repo, run_id}` invokes the configured model and persists a validated task DAG.
3. Review the plan and policy. `run/execute`: `{repo, run_id, policy_path?, runner_backend?}` executes using `local`, `docker` or `kubernetes`. `docker: true` remains supported for compatibility. Autonomous mode rejects local execution.
4. Subscribe to events and inspect `run/get`, `task/list`, `budget/snapshot`.
5. Successful changes remain on the integration branch; the user's branch is not implicitly modified.

Execution rejects empty plans and duplicate starts. Ordinary execution failures persist `failed` and `run.failed`; process termination/restart recovery remains a separate gap.

## Event streaming

The stream replays stored run events after the supplied sequence, then follows new events. It uses bounded pages of 256 events and 250 ms idle polling; heartbeat comments are sent approximately every 15 seconds. Sequence numbers belong to the global ledger and can have gaps within a single run.

```text
id: 42
event: audit
data: {"sequence":42,"event_id":"...","event_type":"run.created",...}

```

Reconnect using `Last-Event-ID: 42` or `?after_sequence=42`. If both are supplied, the larger cursor wins. Negative/invalid cursors fail. Clients should reconnect after network errors using the last processed sequence and deduplicate by event ID. On a storage error the server emits `event: error` and closes; it does not silently skip records. Empty/idle runs keep the connection open, including after completion. Drop/abort the response to unsubscribe.

Both SDKs support authenticated streams (browser `EventSource` cannot set an Authorization header):

```ts
const client = new OpenForgeClient("http://127.0.0.1:8765", token);
const controller = new AbortController();
for await (const event of client.streamEvents(runId, lastSequence, controller.signal)) {
  lastSequence = event.sequence;
  // Persist your cursor after processing the event.
}
// controller.abort() cancels an active subscription.
```

```python
async with OpenForgeClient(api_token=token) as client:
    async for event in client.stream_events(run_id, after_sequence=last_sequence):
        last_sequence = event["sequence"]
```

Generators report failures to the caller; reconnect policy is caller-owned. The web console reconnects after three seconds and displays the latest 500 events. Full history remains available through paginated reads. Disable buffering and configure sufficient idle timeouts in reverse proxies.

`event/verify` validates the global hash chain. It refuses ledgers above its 100,000-event bound rather than reporting a valid partial prefix. Incremental verification is future work.

## Other protocol boundaries

MCP handles tool/data integrations; ACP handles external-agent/editor process interoperability. Neither replaces canonical OpenForge run state. Model token streaming is not implemented by the audit SSE transport. Breaking contract changes require protocol versioning; clients should tolerate additive fields.
