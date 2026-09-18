from __future__ import annotations

import base64
import itertools
from typing import Any, Self

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

        if response.is_error or payload.get("error"):
            error = payload.get("error") or {}
            raise OpenForgeError(
                str(error.get("message") or f"HTTP {response.status_code}")
            )
        if "result" not in payload:
            raise OpenForgeError("daemon response has no result")
        return payload["result"]

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
    ) -> dict[str, Any]:
        return await self.rpc(
            "run/execute",
            {
                "repo": repo,
                "run_id": run_id,
                "policy_path": policy_path,
                "docker": docker,
            },
        )

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

    async def telemetry(self) -> dict[str, Any]:
        return await self.rpc("telemetry/snapshot")

    async def providers(self) -> list[str]:
        result = await self.rpc("model/providers")
        return list(result["providers"])
