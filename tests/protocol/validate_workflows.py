#!/usr/bin/env python3
"""Parse every workflow, including workflows not triggered on the current branch."""
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
paths = sorted((ROOT / ".github/workflows").glob("*.yml"))
if not paths:
    raise SystemExit("no workflows found")
for path in paths:
    # BaseLoader preserves the YAML key `on` instead of YAML 1.1 boolean coercion.
    workflow = yaml.load(path.read_text(), Loader=yaml.BaseLoader)
    if not isinstance(workflow, dict) or not isinstance(workflow.get("jobs"), dict):
        raise SystemExit(f"{path}: workflow must declare jobs")
    if "on" not in workflow:
        raise SystemExit(f"{path}: workflow must declare triggers")
print(f"validated YAML syntax and required sections in {len(paths)} workflows")
