"""Policy-gated MCP server for local Ollama discovery, preflight and model pulls."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import sys
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse
from urllib.request import ProxyHandler, Request, build_opener

HTTP = build_opener(ProxyHandler({}))
DEFAULT_HOST = "http://127.0.0.1:11434"
MIB = 1024 * 1024
MODEL_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/-]*(?::[A-Za-z0-9._-]+)?")


def ollama_host() -> str:
    value = os.environ.get("OLLAMA_HOST", DEFAULT_HOST).strip().rstrip("/")
    parsed = urlparse(value)
    if (
        parsed.scheme != "http"
        or parsed.hostname not in {"127.0.0.1", "localhost", "::1", "host.docker.internal"}
        or parsed.username
        or parsed.password
        or parsed.path not in {"", "/"}
        or parsed.query
        or parsed.fragment
        or (parsed.port or 80) != 11434
    ):
        raise ValueError(
            "OLLAMA_HOST must be a local http://127.0.0.1:11434, localhost, ::1, "
            "or host.docker.internal endpoint"
        )
    return value


def validate_model(value: Any) -> str:
    if not isinstance(value, str) or len(value) > 200 or not MODEL_RE.fullmatch(value):
        raise ValueError("model must be a valid Ollama model identifier")
    return value


def available_memory_bytes() -> int | None:
    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        return None
    return None


def estimate_required_memory(model_size: int, context_tokens: int, headroom_mb: int = 512) -> int:
    return model_size + model_size // 10 + context_tokens * 8 * 1024 + headroom_mb * MIB


def request_json(path: str, payload: dict[str, Any] | None = None, timeout: int = 30) -> Any:
    url = ollama_host() + path
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    request = Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"} if data is not None else {},
        method="POST" if data is not None else "GET",
    )
    try:
        with HTTP.open(request, timeout=timeout) as response:
            raw = response.read(2 * 1024 * 1024 + 1)
    except HTTPError as exc:
        detail = exc.read(4096).decode("utf-8", "replace")
        raise ValueError(f"Ollama returned HTTP {exc.code}: {safe_detail(detail)}") from exc
    except (URLError, TimeoutError, OSError) as exc:
        raise ValueError("Ollama is not reachable on the configured local endpoint") from exc
    if len(raw) > 2 * 1024 * 1024:
        raise ValueError("Ollama response exceeds the 2 MiB plugin limit")
    try:
        return json.loads(raw.decode("utf-8")) if raw else {}
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError("Ollama returned an invalid JSON response") from exc


def safe_detail(raw: str) -> str:
    try:
        value = json.loads(raw)
        if isinstance(value, dict):
            message = value.get("error") or value.get("message")
            if isinstance(message, str):
                return message[:400]
    except json.JSONDecodeError:
        pass
    return "local Ollama request failed"


def inventory() -> dict[str, Any]:
    result = request_json("/api/tags")
    models = result.get("models", []) if isinstance(result, dict) else []
    normalized = []
    for item in models:
        if not isinstance(item, dict):
            continue
        name = item.get("name") or item.get("model")
        if not isinstance(name, str):
            continue
        normalized.append(
            {
                "name": name,
                "model": item.get("model") if isinstance(item.get("model"), str) else name,
                "size": int(item.get("size", 0)) if isinstance(item.get("size", 0), int) else 0,
                "digest": item.get("digest") if isinstance(item.get("digest"), str) else "",
                "modified_at": item.get("modified_at")
                if isinstance(item.get("modified_at"), str)
                else "",
                "details": item.get("details") if isinstance(item.get("details"), dict) else {},
            }
        )
    return {"models": normalized, "available_memory_bytes": available_memory_bytes()}


def preflight(model: str, context_tokens: int) -> dict[str, Any]:
    current = inventory()
    selected = next(
        (
            item
            for item in current["models"]
            if item["name"] == model
            or item["model"] == model
            or (":" not in model and item["name"] == model + ":latest")
        ),
        None,
    )
    if selected is None:
        return {
            "ready": False,
            "installed": False,
            "model": model,
            "context_tokens": context_tokens,
            "available_memory_bytes": current["available_memory_bytes"],
            "estimated_required_memory_bytes": None,
            "message": f"{model} is not installed; pull it before use",
        }
    required = estimate_required_memory(selected["size"], context_tokens)
    available = current["available_memory_bytes"]
    ready = available is None or available >= required
    return {
        "ready": ready,
        "installed": True,
        "model": model,
        "context_tokens": context_tokens,
        "model_size_bytes": selected["size"],
        "available_memory_bytes": available,
        "estimated_required_memory_bytes": required,
        "message": (
            "model is installed and fits the current memory estimate"
            if ready
            else "model is installed but does not fit the current memory estimate"
        ),
    }


def tool_list() -> list[dict[str, Any]]:
    model = {"type": "string", "minLength": 1, "maxLength": 200}
    return [
        {
            "name": "list_models",
            "description": "List locally installed Ollama models and current available system memory.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "show_model",
            "description": "Read Ollama metadata for one locally installed model.",
            "inputSchema": {
                "type": "object",
                "properties": {"model": model},
                "required": ["model"],
                "additionalProperties": False,
            },
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "preflight_model",
            "description": "Check installation and estimated RAM fit for a local Ollama model.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model": model,
                    "context_tokens": {"type": "integer", "minimum": 1, "maximum": 1048576},
                },
                "required": ["model", "context_tokens"],
                "additionalProperties": False,
            },
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "pull_model",
            "description": "Pull a named model into the local Ollama model store.",
            "inputSchema": {
                "type": "object",
                "properties": {"model": model},
                "required": ["model"],
                "additionalProperties": False,
            },
            "annotations": {"readOnlyHint": False, "destructiveHint": False},
        },
    ]


def invoke(name: Any, arguments: Any) -> Any:
    if not isinstance(name, str) or not isinstance(arguments, dict):
        raise ValueError("invalid tool call")
    if name == "list_models":
        if arguments:
            raise ValueError("list_models does not accept arguments")
        return inventory()
    if name == "show_model":
        if set(arguments) != {"model"}:
            raise ValueError("show_model requires only model")
        return request_json("/api/show", {"model": validate_model(arguments["model"])})
    if name == "preflight_model":
        if set(arguments) != {"model", "context_tokens"}:
            raise ValueError("preflight_model requires model and context_tokens")
        model = validate_model(arguments["model"])
        context_tokens = arguments["context_tokens"]
        if type(context_tokens) is not int or not 1 <= context_tokens <= 1_048_576:
            raise ValueError("context_tokens must be an integer from 1 to 1048576")
        return preflight(model, context_tokens)
    if name == "pull_model":
        if set(arguments) != {"model"}:
            raise ValueError("pull_model requires only model")
        model = validate_model(arguments["model"])
        result = request_json("/api/pull", {"model": model, "stream": False}, timeout=1800)
        return {"model": model, "pulled": True, "result": result}
    raise ValueError("unknown Ollama tool")


def handle(request: dict[str, Any]) -> dict[str, Any]:
    method = request.get("method")
    if method == "initialize":
        return {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "openforge-ollama", "version": "0.2.0"},
        }
    if method == "ping":
        return {}
    if method == "tools/list":
        return {"tools": tool_list()}
    if method == "tools/call":
        params = request.get("params", {})
        try:
            result = invoke(params.get("name"), params.get("arguments", {}))
            return {
                "content": [{"type": "text", "text": json.dumps(result)}],
                "structuredContent": result,
                "isError": False,
            }
        except (ValueError, OSError):
            return {
                "content": [
                    {
                        "type": "text",
                        "text": "Ollama call failed. Check the model name, local Ollama service, memory and network policy.",
                    }
                ],
                "isError": True,
            }
    raise ValueError("unsupported method")


def main() -> None:
    for line in sys.stdin:
        request: dict[str, Any] | None = None
        try:
            loaded = json.loads(line)
            if not isinstance(loaded, dict):
                raise ValueError("invalid request")
            request = loaded
            if "id" not in request:
                continue
            response = {"jsonrpc": "2.0", "id": request["id"], "result": handle(request)}
        except (ValueError, TypeError, AttributeError):
            response = {
                "jsonrpc": "2.0",
                "id": request.get("id") if isinstance(request, dict) else None,
                "error": {"code": -32600, "message": "Invalid or unsupported request"},
            }
        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()
