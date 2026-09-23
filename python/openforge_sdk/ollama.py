from __future__ import annotations

import json
from typing import Any, TypeVar, cast

from .client import OpenForgeClient, OpenForgeError

T = TypeVar("T")


def _decode_mcp(result: dict[str, Any]) -> Any:
    if result.get("isError"):
        messages = [
            item.get("text", "")
            for item in result.get("content", [])
            if isinstance(item, dict) and item.get("type") == "text"
        ]
        raise OpenForgeError("\n".join(message for message in messages if message) or "Ollama MCP call failed")
    if "structuredContent" in result:
        return result["structuredContent"]
    for item in result.get("content", []):
        if isinstance(item, dict) and item.get("type") == "text" and isinstance(item.get("text"), str):
            return json.loads(item["text"])
    raise OpenForgeError("Ollama MCP response contained no structured result")


async def list_ollama_models(client: OpenForgeClient) -> dict[str, Any]:
    return cast(dict[str, Any], _decode_mcp(await client.mcp_call_tool("ollama", "list_models", {})))


async def preflight_ollama_model(
    client: OpenForgeClient, model: str, context_tokens: int
) -> dict[str, Any]:
    return cast(
        dict[str, Any],
        _decode_mcp(
            await client.mcp_call_tool(
                "ollama",
                "preflight_model",
                {"model": model, "context_tokens": context_tokens},
            )
        ),
    )


async def show_ollama_model(client: OpenForgeClient, model: str) -> dict[str, Any]:
    return cast(
        dict[str, Any],
        _decode_mcp(await client.mcp_call_tool("ollama", "show_model", {"model": model})),
    )


async def pull_ollama_model(client: OpenForgeClient, model: str) -> dict[str, Any]:
    return cast(
        dict[str, Any],
        _decode_mcp(await client.mcp_call_tool("ollama", "pull_model", {"model": model})),
    )
