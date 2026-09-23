#!/usr/bin/env python3
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
paths = sorted((ROOT / "evals").glob("**/*.yaml")) + sorted((ROOT / "evals").glob("**/*.json"))
if not paths:
    raise SystemExit("no evaluation manifests found")
for path in paths:
    if path.suffix == ".json":
        import json
        value = json.loads(path.read_text(encoding="utf-8"))
    else:
        value = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise SystemExit(f"{path}: eval manifest must be an object")
    if "suite" not in value or "version" not in value or not isinstance(value.get("cases"), list):
        raise SystemExit(f"{path}: suite, version and cases are required")
    if not value["cases"]:
        raise SystemExit(f"{path}: cases cannot be empty")
print(f"validated {len(paths)} evaluation manifests")
