from __future__ import annotations

import pytest

from openforge_sdk.client import OpenForgeError
from openforge_sdk.ollama import list_ollama_models, preflight_ollama_model


class Client:
    async def mcp_call_tool(self, server_name, tool_name, arguments):
        assert server_name == "ollama"
        if tool_name == "list_models":
            return {
                "isError": False,
                "content": [],
                "structuredContent": {
                    "models": [{"name": "qwen2.5-coder:7b"}],
                    "available_memory_bytes": 123,
                },
            }
        if tool_name == "preflight_model":
            assert arguments == {"model": "qwen2.5-coder:7b", "context_tokens": 32768}
            return {
                "isError": False,
                "content": [{"type": "text", "text": '{"ready":true}'}],
            }
        raise AssertionError(tool_name)


@pytest.mark.asyncio
async def test_ollama_sdk_decodes_structured_and_text_mcp_results():
    client = Client()
    inventory = await list_ollama_models(client)  # type: ignore[arg-type]
    assert inventory["models"][0]["name"] == "qwen2.5-coder:7b"
    check = await preflight_ollama_model(client, "qwen2.5-coder:7b", 32768)  # type: ignore[arg-type]
    assert check["ready"] is True


@pytest.mark.asyncio
async def test_ollama_sdk_surfaces_mcp_errors():
    class ErrorClient:
        async def mcp_call_tool(self, *_args, **_kwargs):
            return {"isError": True, "content": [{"type": "text", "text": "unavailable"}]}

    with pytest.raises(OpenForgeError, match="unavailable"):
        await list_ollama_models(ErrorClient())  # type: ignore[arg-type]
