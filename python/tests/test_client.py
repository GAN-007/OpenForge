import httpx
import pytest

from openforge_sdk.client import OpenForgeClient, OpenForgeError


@pytest.mark.asyncio
async def test_rpc_surfaces_daemon_error(monkeypatch):
    async def fake_post(*args, **kwargs):
        return httpx.Response(
            400,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32000, "message": "bad run"},
            },
            request=httpx.Request("POST", "http://x"),
        )

    client = OpenForgeClient()
    monkeypatch.setattr(client._client, "post", fake_post)

    with pytest.raises(OpenForgeError, match="bad run"):
        await client.get_run("x")

    await client.close()
