from __future__ import annotations

import itertools
from typing import Any

import httpx


class OpenForgeError(RuntimeError):
    pass


class OpenForgeClient:
    def __init__(
        self,
        base_url: str = "http://127.0.0.1:8765",
        timeout: float = 30.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self._ids = itertools.count(1)
        self._client = httpx.AsyncClient(base_url=self.base_url, timeout=timeout)

    async def __aenter__(self) -> "OpenForgeClient":
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.close()

    async def close(self) -> None:
        await self._client.aclose()

    async def rpc(self, method: str, params: dict[str, Any] | None = None) -> Any:
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

    async def get_run(self, run_id: str) -> dict[str, Any] | None:
        return await self.rpc("run/get", {"run_id": run_id})

    async def list_tasks(self, run_id: str) -> list[dict[str, Any]]:
        return await self.rpc("task/list", {"run_id": run_id})

    async def list_events(
        self, run_id: str, after_sequence: int = 0
    ) -> list[dict[str, Any]]:
        return await self.rpc(
            "event/list",
            {"run_id": run_id, "after_sequence": after_sequence},
        )

    async def budget(self, run_id: str) -> dict[str, Any]:
        return await self.rpc("budget/get", {"run_id": run_id})
