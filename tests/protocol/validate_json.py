#!/usr/bin/env python3
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

SCHEMAS = [
    ROOT / "schemas/events/event.schema.json",
    ROOT / "schemas/plugin/plugin.schema.json",
    ROOT / "schemas/task/task.schema.json",
]

MANIFESTS = list((ROOT / "plugins").glob("**/plugin.json"))

for path in SCHEMAS + MANIFESTS:
    with path.open("r", encoding="utf-8") as handle:
        json.load(handle)

for path in MANIFESTS:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != "openforge.plugin/v1":
        raise SystemExit(f"{path}: unsupported plugin schema")
    if not data.get("id") or not data.get("entrypoint"):
        raise SystemExit(f"{path}: id and entrypoint are required")

print(f"validated {len(SCHEMAS)} schemas and {len(MANIFESTS)} plugin manifests")
