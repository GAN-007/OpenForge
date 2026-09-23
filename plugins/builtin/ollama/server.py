"""Local Ollama MCP server for model discovery, inspection and explicit pulls."""
from __future__ import annotations

import json
import os
import re
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse
from urllib.request import Request, build_opener, ProxyHandler

MODEL = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}$")
HTTP = build_opener(ProxyHandler({}))


def base_url() -> str:
    value = os.environ.get("OLLAMA_URL", "http://127.0.0.1:11434").rstrip("/")
    parsed = urlparse(value)
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("OLLAMA_URL must be a loopback http endpoint")
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("OLLAMA_URL must not contain credentials, query or fragment")
    return value


def request_json(path: str, payload: dict | None = None, timeout: int = 30):
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    request = Request(
        base_url() + path,
        data=data,
        headers={"content-type": "application/json"} if data is not None else {},
        method="POST" if data is not None else "GET",
    )
    try:
        with HTTP.open(request, timeout=timeout) as response:
            raw = response.read(8 * 1024 * 1024)
    except HTTPError as error:
        body = error.read(4096).decode("utf-8", "replace")
        raise ValueError(f"Ollama returned HTTP {error.code}: {body}") from error
    except (URLError, OSError) as error:
        raise ValueError("Ollama is not reachable on the configured loopback endpoint") from error
    return json.loads(raw) if raw.strip() else {}


def available_memory_bytes() -> int | None:
    try:
        for line in open("/proc/meminfo", encoding="utf-8"):
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        return None
    return None


def model_name(arguments: dict) -> str:
    name = arguments.get("model")
    if not isinstance(name, str) or not MODEL.fullmatch(name):
        raise ValueError("model must be a valid Ollama model identifier")
    return name


def tool_list():
    return [
        {
            "name": "list_models",
            "description": "List models installed in the local Ollama runtime.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "show_model",
            "description": "Inspect metadata for one installed local Ollama model.",
            "inputSchema": {
                "type": "object",
                "properties": {"model": {"type": "string", "minLength": 1}},
                "required": ["model"],
                "additionalProperties": False,
            },
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "preflight_model",
            "description": "Check whether one local Ollama model is installed and fits current RAM headroom.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model": {"type": "string", "minLength": 1},
                    "context_tokens": {"type": "integer", "minimum": 1, "maximum": 2097152},
                },
                "required": ["model", "context_tokens"],
                "additionalProperties": False,
            },
            "annotations": {"readOnlyHint": True},
        },
        {
            "name": "pull_model",
            "description": "Pull a model into the local Ollama runtime. This can download several gigabytes.",
            "inputSchema": {
                "type": "object",
                "properties": {"model": {"type": "string", "minLength": 1}},
                "required": ["model"],
                "additionalProperties": False,
            },
            "annotations": {"readOnlyHint": False, "destructiveHint": False},
        },
    ]


def invoke(name: str, arguments: dict):
    if not isinstance(arguments, dict):
        raise ValueError("arguments must be an object")
    if name == "list_models":
        if arguments:
            raise ValueError("list_models takes no arguments")
        result = request_json("/api/tags")
        return {
            "models": result.get("models", []),
            "available_memory_bytes": available_memory_bytes(),
        }
    if name == "show_model":
        model = model_name(arguments)
        return request_json("/api/show", {"model": model})
    if name == "preflight_model":
        model = model_name(arguments)
        context_tokens = arguments.get("context_tokens")
        if type(context_tokens) is not int or not 1 <= context_tokens <= 2097152:
            raise ValueError("context_tokens must be an integer from 1 to 2097152")
        inventory = request_json("/api/tags").get("models", [])
        candidate = next(
            (
                item for item in inventory
                if item.get("name") == model or item.get("model") == model
            ),
            None,
        )
        available = available_memory_bytes()
        if candidate is None:
            return {
                "ready": False,
                "installed": False,
                "model": model,
                "context_tokens": context_tokens,
                "available_memory_bytes": available,
                "estimated_required_memory_bytes": None,
                "message": f"{model} is not installed; pull it before use",
            }
        size = int(candidate.get("size") or 0)
        required = ((size * 115 + 99) // 100) + 512 * 1024 * 1024 if size else None
        ready = required is None or available is None or available >= required
        return {
            "ready": ready,
            "installed": True,
            "model": model,
            "context_tokens": context_tokens,
            "model_size_bytes": size or None,
            "available_memory_bytes": available,
            "estimated_required_memory_bytes": required,
            "message": (
                "model passed local memory preflight"
                if ready
                else f"insufficient free RAM: {available} bytes available, approximately {required} bytes required"
            ),
        }
    if name == "pull_model":
        model = model_name(arguments)
        result = request_json("/api/pull", {"model": model, "stream": False}, timeout=900)
        return {
            "model": model,
            "status": result.get("status", "unknown"),
            "completed": result.get("status") == "success",
        }
    raise ValueError("unknown tool")


def handle(request: dict):
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
        result = invoke(params.get("name"), params.get("arguments", {}))
        return {
            "content": [{"type": "text", "text": json.dumps(result, separators=(",", ":"))}],
            "structuredContent": result,
            "isError": False,
        }
    raise ValueError("unsupported method")


def main() -> None:
    for line in sys.stdin:
        request = None
        try:
            request = json.loads(line)
            if not isinstance(request, dict):
                raise ValueError("invalid request")
            if "id" not in request:
                continue
            response = {"jsonrpc": "2.0", "id": request["id"], "result": handle(request)}
        except (ValueError, TypeError, AttributeError, json.JSONDecodeError):
            response = {
                "jsonrpc": "2.0",
                "id": request.get("id") if isinstance(request, dict) else None,
                "error": {"code": -32600, "message": "Invalid or unsupported request"},
            }
        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()
