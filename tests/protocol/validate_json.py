#!/usr/bin/env python3
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCHEMAS = sorted((ROOT / "schemas").glob("**/*.json"))
MANIFESTS = sorted((ROOT / "plugins").glob("**/plugin.json"))

if not SCHEMAS:
    raise SystemExit("no JSON schemas found")

for path in SCHEMAS + MANIFESTS:
    with path.open("r", encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise SystemExit(f"{path}: top-level JSON must be an object")

for path in MANIFESTS:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != "openforge.plugin/v2":
        raise SystemExit(f"{path}: unsupported plugin schema {data.get('schema')!r}")
    if not data.get("id") or not data.get("entrypoint") or not data.get("api"):
        raise SystemExit(f"{path}: id, entrypoint and api are required")
    if not isinstance(data.get("capabilities"), dict):
        raise SystemExit(f"{path}: capabilities must be an object")

print(f"validated {len(SCHEMAS)} schemas and {len(MANIFESTS)} plugin manifests")
