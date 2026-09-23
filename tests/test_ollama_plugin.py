from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from types import ModuleType

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / "plugins" / "builtin" / "ollama" / "server.py"


def load_plugin() -> ModuleType:
    spec = importlib.util.spec_from_file_location("openforge_ollama_plugin", PLUGIN)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Response:
    def __init__(self, value):
        self.value = value

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return None

    def read(self, _limit):
        return json.dumps(self.value).encode()


def test_tools_are_bounded_and_model_identifiers_are_validated(monkeypatch):
    plugin = load_plugin()
    names = {tool["name"] for tool in plugin.tool_list()}
    assert names == {"list_models", "show_model", "preflight_model", "pull_model"}
    for invalid in ["", "../../secret", "model name", "https://example.com/model"]:
        try:
            plugin.validate_model(invalid)
        except ValueError:
            pass
        else:
            raise AssertionError(f"invalid model accepted: {invalid}")

    monkeypatch.setenv("OLLAMA_HOST", "https://remote.example.com:11434")
    try:
        plugin.ollama_host()
    except ValueError:
        pass
    else:
        raise AssertionError("non-local OLLAMA_HOST must be rejected")


def test_inventory_preflight_and_pull_use_local_ollama_api(monkeypatch):
    plugin = load_plugin()
    calls = []

    def fake_open(request, timeout):
        calls.append((request.full_url, request.get_method(), request.data, timeout))
        if request.full_url.endswith("/api/tags"):
            return Response(
                {
                    "models": [
                        {
                            "name": "qwen2.5-coder:7b",
                            "model": "qwen2.5-coder:7b",
                            "size": 1024,
                            "digest": "abc",
                        }
                    ]
                }
            )
        if request.full_url.endswith("/api/pull"):
            return Response({"status": "success"})
        raise AssertionError(request.full_url)

    monkeypatch.setattr(plugin.HTTP, "open", fake_open)
    monkeypatch.setattr(plugin, "available_memory_bytes", lambda: 4 * 1024 * 1024 * 1024)

    listed = plugin.invoke("list_models", {})
    assert listed["models"][0]["name"] == "qwen2.5-coder:7b"
    check = plugin.invoke(
        "preflight_model", {"model": "qwen2.5-coder:7b", "context_tokens": 4096}
    )
    assert check["ready"] is True
    assert check["installed"] is True
    pulled = plugin.invoke("pull_model", {"model": "qwen2.5-coder:7b"})
    assert pulled["pulled"] is True
    assert any(url.endswith("/api/pull") and method == "POST" for url, method, _, _ in calls)
