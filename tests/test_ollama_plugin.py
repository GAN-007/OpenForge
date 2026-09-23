import importlib.util
import os
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "openforge_ollama_plugin", ROOT / "plugins/builtin/ollama/server.py"
)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MODULE)


class OllamaPluginTests(unittest.TestCase):
    def test_tools_are_complete_and_model_validation_is_strict(self):
        self.assertEqual(
            [tool["name"] for tool in MODULE.tool_list()],
            ["list_models", "show_model", "pull_model"],
        )
        self.assertEqual(MODULE.model_name({"model": "qwen2.5-coder:7b"}), "qwen2.5-coder:7b")
        for value in ("", "../model", "bad model", "x" * 129):
            with self.assertRaises(ValueError):
                MODULE.model_name({"model": value})

    def test_endpoint_is_loopback_only(self):
        with patch.dict(os.environ, {"OLLAMA_URL": "https://example.com"}, clear=False):
            with self.assertRaises(ValueError):
                MODULE.base_url()
        with patch.dict(os.environ, {"OLLAMA_URL": "http://127.0.0.1:11434"}, clear=False):
            self.assertEqual(MODULE.base_url(), "http://127.0.0.1:11434")


if __name__ == "__main__":
    unittest.main()
