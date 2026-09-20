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


@pytest.mark.asyncio
@pytest.mark.parametrize("payload", [[], None, {"jsonrpc": "2.0", "id": 99, "result": []}])
async def test_rpc_rejects_invalid_envelopes(monkeypatch, payload):
    async def fake_post(*args, **kwargs):
        return httpx.Response(200, json=payload, request=httpx.Request("POST", "http://x"))

    async with OpenForgeClient() as client:
        monkeypatch.setattr(client._client, "post", fake_post)
        with pytest.raises(OpenForgeError, match="invalid|mismatched"):
            await client.list_runs()


@pytest.mark.asyncio
async def test_authenticated_event_replay_and_heartbeats():
    def respond(request):
        assert request.headers["authorization"] == "Bearer secret"
        assert request.url.params["after_sequence"] == "4"
        return httpx.Response(200, text=': ping\r\n\r\nevent: audit\r\ndata: {"sequence":5}\r\n\r\n')

    async with OpenForgeClient(api_token="secret") as client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(
            base_url="http://test", headers={"authorization": "Bearer secret"},
            transport=httpx.MockTransport(respond),
        )
        assert [event async for event in client.stream_events("run", 4)] == [{"sequence": 5}]


@pytest.mark.asyncio
async def test_stream_reports_server_error():
    async with OpenForgeClient() as client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(
            base_url="http://test",
            transport=httpx.MockTransport(lambda _: httpx.Response(200, text="event: error\ndata: failed\n\n")),
        )
        with pytest.raises(OpenForgeError, match="failed"):
            async for _ in client.stream_events("run"):
                pass
