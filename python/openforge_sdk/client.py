from __future__ import annotations

import base64
import itertools
import json
import sys
from collections.abc import AsyncIterator
from typing import Any

if sys.version_info >= (3, 11):
    from typing import Self
else:
    from typing_extensions import Self

import httpx


class OpenForgeError(RuntimeError):
    pass


class OpenForgeClient:
    def __init__(
        self,
        base_url: str = "http://127.0.0.1:8765",
        timeout: float = 30.0,
        api_token: str | None = None,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self._ids = itertools.count(1)
        headers = {"authorization": f"Bearer {api_token}"} if api_token else None
        self._client = httpx.AsyncClient(
            base_url=self.base_url,
            timeout=timeout,
            headers=headers,
        )

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.close()

    async def close(self) -> None:
        await self._client.aclose()

    async def rpc(
        self,
        method: str,
        params: dict[str, Any] | None = None,
    ) -> Any:
        request_id = next(self._ids)
        response = await self._client.post(
            "/v1/rpc",
            json={
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params or {},
            },
        )
        try:
            payload = response.json()
        except ValueError as exc:
            raise OpenForgeError(
                f"invalid daemon response: HTTP {response.status_code}"
            ) from exc

        if not isinstance(payload, dict) or payload.get("jsonrpc") != "2.0" or payload.get("id") != request_id:
            raise OpenForgeError("invalid or mismatched daemon response")
        if response.is_error or payload.get("error"):
            error = payload.get("error") or {}
            raise OpenForgeError(
                str(error.get("message") or f"HTTP {response.status_code}")
            )
        if "result" not in payload:
            raise OpenForgeError("daemon response has no result")
        return payload["result"]

    async def gateway_status(self) -> dict[str, Any]:
        return await self.rpc("gateway/status")

    async def connect_gateway(self, api_key: str) -> dict[str, Any]:
        """Test and use Sevi for this daemon session; the key is not persisted."""
        return await self.rpc("gateway/connect", {"api_key": api_key})

    async def disconnect_gateway(self) -> dict[str, Any]:
        return await self.rpc("gateway/disconnect")

    async def list_models(self) -> list[dict[str, Any]]:
        return await self.rpc("model/list")

    async def initialize(self) -> dict[str, Any]:
        return await self.rpc("initialize")

    async def create_run(
        self,
        repo: str,
        objective: str,
        *,
        autonomy: str = "suggest",
        budget_usd: float = 10.0,
    ) -> dict[str, Any]:
        return await self.rpc(
            "run/create",
            {
                "repo": repo,
                "objective": objective,
                "autonomy": autonomy,
                "budget_usd": budget_usd,
            },
        )

    async def plan_run(self, repo: str, run_id: str) -> list[dict[str, Any]]:
        return await self.rpc("run/plan", {"repo": repo, "run_id": run_id})

    async def execute_run(
        self,
        repo: str,
        run_id: str,
        *,
        policy_path: str = "config/policies/development.yaml",
        docker: bool = False,
        runner_backend: str | None = None,
    ) -> dict[str, Any]:
        return await self.rpc(
            "run/execute",
            {
                "repo": repo,
                "run_id": run_id,
                "policy_path": policy_path,
                "docker": docker,
                "runner_backend": runner_backend,
            },
        )

    async def list_runs(self, limit: int = 100, offset: int = 0) -> list[dict[str, Any]]:
        return await self.rpc("run/list", {"limit": limit, "offset": offset})

    async def stream_events(self, run_id: str, after_sequence: int = 0) -> AsyncIterator[dict[str, Any]]:
        """Replay and follow audit events; resume with the last yielded sequence."""
        from urllib.parse import quote

        async with self._client.stream(
            "GET", f"/v1/runs/{quote(run_id, safe='')}/events/stream",
            params={"after_sequence": after_sequence},
            headers={"accept": "text/event-stream"},
            timeout=httpx.Timeout(30.0, read=None),
        ) as response:
            if response.is_error:
                raise OpenForgeError(f"event stream failed: HTTP {response.status_code}")
            event_type = ""
            data: list[str] = []
            async for line in response.aiter_lines():
                if not line:
                    if event_type == "error":
                        raise OpenForgeError("\n".join(data))
                    if data and event_type == "audit":
                        yield json.loads("\n".join(data))
                    event_type, data = "", []
                elif line.startswith("event:"):
                    event_type = line[6:].removeprefix(" ")
                elif line.startswith("data:"):
                    data.append(line[5:].removeprefix(" "))

    async def get_run(self, run_id: str) -> dict[str, Any] | None:
        return await self.rpc("run/get", {"run_id": run_id})

    async def list_tasks(self, run_id: str) -> list[dict[str, Any]]:
        return await self.rpc("task/list", {"run_id": run_id})

    async def list_events(
        self,
        run_id: str,
        after_sequence: int = 0,
        limit: int = 500,
    ) -> list[dict[str, Any]]:
        return await self.rpc(
            "event/list",
            {
                "run_id": run_id,
                "after_sequence": after_sequence,
                "limit": limit,
            },
        )

    async def verify_events(self) -> dict[str, Any]:
        return await self.rpc("event/verify")

    async def budget(self, run_id: str) -> dict[str, Any]:
        return await self.rpc("budget/get", {"run_id": run_id})

    async def repository_index(self, repo: str) -> dict[str, Any]:
        return await self.rpc("repository/index", {"repo": repo})

    async def search(
        self,
        repo: str,
        query: str,
        limit: int = 25,
    ) -> dict[str, Any]:
        return await self.rpc(
            "search/query",
            {"repo": repo, "query": query, "limit": limit},
        )

    async def symbols(
        self,
        repo: str,
        query: str,
        limit: int = 25,
    ) -> dict[str, Any]:
        return await self.rpc(
            "symbols/query",
            {"repo": repo, "query": query, "limit": limit},
        )

    async def symbol_graph(self, repo: str) -> dict[str, Any]:
        return await self.rpc("symbols/graph", {"repo": repo})

    async def put_artifact_bytes(
        self,
        content: bytes,
        *,
        media_type: str = "application/octet-stream",
        source: str = "python-sdk",
        metadata: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return await self.rpc(
            "artifact/put",
            {
                "base64": base64.b64encode(content).decode("ascii"),
                "media_type": media_type,
                "source": source,
                "metadata": metadata or {},
            },
        )

    async def get_artifact_bytes(self, sha256: str) -> tuple[dict[str, Any], bytes]:
        result = await self.rpc("artifact/get", {"sha256": sha256})
        return result["descriptor"], base64.b64decode(result["base64"], validate=True)

    async def artifact_descriptor(self, sha256: str) -> dict[str, Any]:
        return await self.rpc("artifact/descriptor", {"sha256": sha256})

    async def delete_artifact(self, sha256: str) -> bool:
        result = await self.rpc("artifact/delete", {"sha256": sha256})
        return bool(result["deleted"])

    async def evaluate_policy(
        self,
        policy_path: str,
        capability: str,
        *,
        subject: str = "",
        argv: list[str] | None = None,
    ) -> str:
        params: dict[str, Any] = {
            "policy_path": policy_path,
            "capability": capability,
            "subject": subject,
        }
        if argv is not None:
            params["argv"] = argv
        result = await self.rpc("policy/evaluate", params)
        return str(result["decision"])

    async def lease_secret(
        self,
        secret_name: str,
        audience: str,
        *,
        ttl_seconds: int = 60,
        policy_path: str = "config/policies/development.yaml",
    ) -> dict[str, Any]:
        return await self.rpc(
            "secret/lease",
            {
                "secret_name": secret_name,
                "audience": audience,
                "ttl_seconds": ttl_seconds,
                "policy_path": policy_path,
            },
        )

    async def revoke_secret(self, lease_id: str) -> bool:
        result = await self.rpc("secret/revoke", {"lease_id": lease_id})
        return bool(result["revoked"])

    async def list_secret_leases(self) -> list[dict[str, Any]]:
        return await self.rpc("secret/list")

    async def acp_spawn(
        self,
        program: str,
        *,
        args: list[str] | None = None,
        cwd: str | None = None,
        environment: dict[str, str] | None = None,
        timeout_seconds: int = 120,
        policy_path: str = "config/policies/development.yaml",
    ) -> str:
        result = await self.rpc(
            "acp/spawn",
            {
                "program": program,
                "args": args or [],
                "cwd": cwd,
                "environment": environment or {},
                "timeout_seconds": timeout_seconds,
                "policy_path": policy_path,
            },
        )
        return str(result["process_id"])

    async def acp_request(
        self,
        process_id: str,
        method: str,
        params: Any = None,
    ) -> Any:
        result = await self.rpc(
            "acp/request",
            {"process_id": process_id, "method": method, "params": params},
        )
        return result["result"]

    async def acp_notify(
        self,
        process_id: str,
        method: str,
        params: Any = None,
    ) -> None:
        await self.rpc(
            "acp/notify",
            {"process_id": process_id, "method": method, "params": params},
        )

    async def acp_close(self, process_id: str) -> dict[str, Any]:
        return await self.rpc("acp/close", {"process_id": process_id})

    async def acp_list(self) -> list[dict[str, Any]]:
        return await self.rpc("acp/list")

    async def mcp_list_tools(
        self,
        server_name: str,
        *,
        policy_path: str = "config/policies/development.yaml",
    ) -> list[dict[str, Any]]:
        return await self.rpc(
            "mcp/list_tools",
            {"server_name": server_name, "policy_path": policy_path},
        )

    async def mcp_call_tool(
        self,
        server_name: str,
        tool_name: str,
        arguments: Any,
        *,
        policy_path: str = "config/policies/development.yaml",
    ) -> dict[str, Any]:
        return await self.rpc(
            "mcp/call_tool",
            {
                "server_name": server_name,
                "tool_name": tool_name,
                "arguments": arguments,
                "policy_path": policy_path,
            },
        )

    async def mcp_list_resources(
        self,
        server_name: str,
        *,
        policy_path: str = "config/policies/development.yaml",
    ) -> Any:
        return await self.rpc(
            "mcp/list_resources",
            {"server_name": server_name, "policy_path": policy_path},
        )

    async def mcp_read_resource(
        self,
        server_name: str,
        uri: str,
        *,
        policy_path: str = "config/policies/development.yaml",
    ) -> Any:
        return await self.rpc(
            "mcp/read_resource",
            {
                "server_name": server_name,
                "uri": uri,
                "policy_path": policy_path,
            },
        )

    async def mcp_list_prompts(
        self,
        server_name: str,
        *,
        policy_path: str = "config/policies/development.yaml",
    ) -> Any:
        return await self.rpc(
            "mcp/list_prompts",
            {"server_name": server_name, "policy_path": policy_path},
        )

    async def budget_reserve(
        self,
        run_id: str,
        estimated_usd: float,
    ) -> dict[str, Any]:
        return await self.rpc(
            "budget/reserve",
            {"run_id": run_id, "estimated_usd": estimated_usd},
        )

    async def budget_settle(
        self,
        reservation_id: str,
        actual_usd: float,
    ) -> dict[str, Any]:
        return await self.rpc(
            "budget/settle",
            {
                "reservation_id": reservation_id,
                "actual_usd": actual_usd,
            },
        )

    async def budget_snapshot(self, run_id: str) -> dict[str, Any]:
        return await self.rpc("budget/snapshot", {"run_id": run_id})

    async def artifact_stream_begin(
        self,
        *,
        media_type: str = "application/octet-stream",
        source: str = "python-sdk-stream",
    ) -> str:
        result = await self.rpc(
            "artifact/stream/begin",
            {"media_type": media_type, "source": source},
        )
        return str(result["upload_id"])

    async def artifact_stream_chunk(self, upload_id: str, content: bytes) -> int:
        result = await self.rpc(
            "artifact/stream/chunk",
            {
                "upload_id": upload_id,
                "base64": base64.b64encode(content).decode("ascii"),
            },
        )
        return int(result["bytes_written"])

    async def artifact_stream_commit(
        self,
        upload_id: str,
        metadata: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return await self.rpc(
            "artifact/stream/commit",
            {"upload_id": upload_id, "metadata": metadata or {}},
        )

    async def artifact_stream_abort(self, upload_id: str) -> bool:
        result = await self.rpc(
            "artifact/stream/abort",
            {"upload_id": upload_id},
        )
        return bool(result["aborted"])

    async def list_plugins(self) -> list[dict[str, Any]]:
        return await self.rpc("plugins/list")

    async def validate_plugin_capability(
        self,
        plugin_id: str,
        capability: str,
    ) -> dict[str, Any]:
        return await self.rpc(
            "plugins/capability/validate",
            {"plugin_id": plugin_id, "capability": capability},
        )

    async def telemetry(self) -> dict[str, Any]:
        return await self.rpc("telemetry/snapshot")

    async def providers(self) -> list[str]:
        result = await self.rpc("model/providers")
        return list(result["providers"])
