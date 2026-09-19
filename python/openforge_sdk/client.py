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
            raise OpenForgeError(f"invalid daemon response: HTTP {response.status_code}") from exc

        if response.is_error or payload.get("error"):
            error = payload.get("error") or {}
            raise OpenForgeError(str(error.get("message") or f"HTTP {response.status_code}"))
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

    async def read_file(self, repo: str, path: str) -> dict[str, Any]:
        return await self.rpc("workspace/file-read", {"repo": repo, "path": path})

    async def write_file(self, repo: str, path: str, content: str) -> dict[str, Any]:
        return await self.rpc(
            "workspace/file-write", {"repo": repo, "path": path, "content": content}
        )

    async def git_status(self, repo: str) -> dict[str, Any]:
        return await self.rpc("git/status", {"repo": repo})

    async def knowledge_search(self, repo: str, query: str, limit: int = 25) -> dict[str, Any]:
        return await self.rpc("knowledge/search", {"repo": repo, "query": query, "limit": limit})

    async def predict_edits(self, prediction: dict[str, Any]) -> dict[str, Any]:
        return await self.rpc("edit/predict", prediction)

    async def language_servers(self) -> list[dict[str, Any]]:
        return await self.rpc("lsp/list")

    async def start_language_server(self, name: str, repo: str) -> dict[str, Any]:
        return await self.rpc("lsp/start", {"name": name, "repo": repo})

    async def lsp_request(self, session_id: str, method: str, params: Any) -> Any:
        return await self.rpc(
            "lsp/request",
            {"session_id": session_id, "request_method": method, "request_params": params},
        )

    async def debug_adapters(self) -> list[dict[str, Any]]:
        return await self.rpc("dap/list")

    async def start_debug_adapter(self, name: str, repo: str) -> dict[str, Any]:
        return await self.rpc("dap/start", {"name": name, "repo": repo})

    async def dap_request(self, session_id: str, command: str, arguments: Any | None = None) -> Any:
        return await self.rpc(
            "dap/request",
            {"session_id": session_id, "command": command, "arguments": arguments or {}},
        )

    async def spawn_terminal(
        self,
        repo: str,
        program: str,
        *,
        args: list[str] | None = None,
        cwd: str = ".",
        environment: dict[str, str] | None = None,
        rows: int = 30,
        cols: int = 120,
    ) -> dict[str, Any]:
        return await self.rpc(
            "terminal/spawn",
            {
                "repo": repo,
                "program": program,
                "args": args or [],
                "cwd": cwd,
                "environment": environment or {},
                "rows": rows,
                "cols": cols,
            },
        )

    async def register_worker(
        self,
        name: str,
        *,
        endpoint: str | None = None,
        capabilities: list[str] | None = None,
        labels: list[str] | None = None,
    ) -> dict[str, Any]:
        return await self.rpc(
            "worker/register",
            {
                "name": name,
                "endpoint": endpoint,
                "capabilities": capabilities or [],
                "labels": labels or [],
            },
        )

    async def submit_job(
        self,
        run_id: str,
        *,
        task_id: str | None = None,
        payload: Any = None,
        required_capabilities: list[str] | None = None,
        max_attempts: int = 3,
    ) -> dict[str, Any]:
        return await self.rpc(
            "worker/submit",
            {
                "run_id": run_id,
                "task_id": task_id,
                "payload": payload or {},
                "required_capabilities": required_capabilities or [],
                "max_attempts": max_attempts,
            },
        )

    async def claim_job(self, worker_id: str, lease_seconds: int = 60) -> dict[str, Any] | None:
        return await self.rpc(
            "worker/claim", {"worker_id": worker_id, "lease_seconds": lease_seconds}
        )

    async def checkpoint_job(
        self, job_id: str, lease_token: str, sequence: int, state: Any
    ) -> dict[str, Any]:
        return await self.rpc(
            "worker/checkpoint",
            {
                "job_id": job_id,
                "lease_token": lease_token,
                "sequence": sequence,
                "state": state,
            },
        )

    async def put_rich_memory(self, memory: dict[str, Any]) -> dict[str, Any]:
        return await self.rpc("memory/rich-put", memory)

    async def search_rich_memory(
        self,
        *,
        scope: str | None = None,
        query: str | None = None,
        query_embedding: list[float] | None = None,
        repository_fingerprint: str | None = None,
        limit: int = 25,
    ) -> list[dict[str, Any]]:
        return await self.rpc(
            "memory/rich-search",
            {
                "scope": scope,
                "query": query,
                "query_embedding": query_embedding,
                "repository_fingerprint": repository_fingerprint,
                "limit": limit,
            },
        )

    async def create_thread(
        self, run_id: str, title: str, task_id: str | None = None
    ) -> dict[str, Any]:
        return await self.rpc(
            "thread/create", {"run_id": run_id, "task_id": task_id, "title": title}
        )

    async def thread_messages(
        self, thread_id: str, after_sequence: int = 0, limit: int = 500
    ) -> list[dict[str, Any]]:
        return await self.rpc(
            "thread/messages",
            {"thread_id": thread_id, "after_sequence": after_sequence, "limit": limit},
        )

    async def queue_instruction(self, thread_id: str, content: str) -> dict[str, Any]:
        return await self.rpc(
            "thread/instruction-queue", {"thread_id": thread_id, "content": content}
        )

    async def request_approval(
        self, thread_id: str, capability: str, subject: str, reason: str
    ) -> dict[str, Any]:
        return await self.rpc(
            "approval/request",
            {
                "thread_id": thread_id,
                "capability": capability,
                "subject": subject,
                "reason": reason,
            },
        )

    async def database_introspect(
        self, repo: str, engine: str, environment: dict[str, str] | None = None
    ) -> dict[str, Any]:
        return await self.rpc(
            "database/introspect",
            {"repo": repo, "engine": engine, "environment": environment or {}},
        )

    async def docker_inventory(self, repo: str) -> Any:
        return await self.rpc("devops/docker-inventory", {"repo": repo})

    async def kubernetes_inventory(self, repo: str) -> Any:
        return await self.rpc("devops/kubernetes-inventory", {"repo": repo})

    async def terraform_plan(self, repo: str, directory: str = ".") -> Any:
        return await self.rpc("devops/terraform-plan", {"repo": repo, "directory": directory})

    async def create_debug_session(
        self, run_id: str, issue: str, task_id: str | None = None
    ) -> dict[str, Any]:
        return await self.rpc(
            "debug/create", {"run_id": run_id, "task_id": task_id, "issue": issue}
        )

    async def load_plugin(self, path: str) -> dict[str, Any]:
        return await self.rpc("plugin/load", {"path": path})

    async def invoke_plugin(self, plugin_id: str, invocation: dict[str, Any]) -> Any:
        return await self.rpc("plugin/invoke", {"plugin_id": plugin_id, "invocation": invocation})

    async def providers(self) -> list[str]:
        result = await self.rpc("model/providers")
        return list(result["providers"])
