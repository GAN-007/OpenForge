#!/usr/bin/env python3
from __future__ import annotations

import json
import re
from pathlib import Path

import yaml

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

plugin_schema = json.loads((ROOT / "schemas/plugin/plugin.schema.json").read_text())

for path in MANIFESTS:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != "openforge.plugin/v2":
        raise SystemExit(f"{path}: unsupported plugin schema {data.get('schema')!r}")
    if not re.fullmatch(plugin_schema["properties"]["id"]["pattern"], data.get("id", "")):
        raise SystemExit(f"{path}: invalid reverse-DNS plugin id")
    if not data.get("id") or not data.get("entrypoint") or not data.get("api"):
        raise SystemExit(f"{path}: id, entrypoint and api are required")
    if not isinstance(data.get("capabilities"), dict):
        raise SystemExit(f"{path}: capabilities must be an object")


def validate_schema(value, schema, path):
    expected = schema.get("type")
    if expected:
        actual = {
            "object": isinstance(value, dict),
            "array": isinstance(value, list),
            "string": isinstance(value, str),
            "integer": type(value) is int,
            "number": type(value) in (int, float),
            "boolean": type(value) is bool,
        }.get(expected)
        if actual is not True:
            raise SystemExit(f"{path}: expected {expected}")
    if "enum" in schema and value not in schema["enum"]:
        raise SystemExit(f"{path}: value {value!r} not in enum")
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            raise SystemExit(f"{path}: below minimum {schema['minimum']}")
        if "maximum" in schema and value > schema["maximum"]:
            raise SystemExit(f"{path}: above maximum {schema['maximum']}")
    if isinstance(value, str) and len(value) < schema.get("minLength", 0):
        raise SystemExit(f"{path}: string shorter than minLength")
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            raise SystemExit(f"{path}: array shorter than minItems")
        if "items" in schema:
            for index, item in enumerate(value):
                validate_schema(item, schema["items"], f"{path}[{index}]")
    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                raise SystemExit(f"{path}.{key}: required")
        properties = schema.get("properties", {})
        additional = schema.get("additionalProperties", True)
        for key, item in value.items():
            if key in properties:
                validate_schema(item, properties[key], f"{path}.{key}")
            elif additional is False:
                raise SystemExit(f"{path}.{key}: unknown property")
            elif isinstance(additional, dict):
                validate_schema(item, additional, f"{path}.{key}")


provider_schema = json.loads((ROOT / "schemas/model-provider.schema.json").read_text())
config = yaml.safe_load((ROOT / "openforge.yaml").read_text(encoding="utf-8"))
providers = config.get("providers") if isinstance(config, dict) else None
if not isinstance(providers, list) or not providers:
    raise SystemExit("openforge.yaml: providers must be a non-empty list")
for index, provider in enumerate(providers):
    validate_schema(provider, provider_schema, f"openforge.yaml.providers[{index}]")

print(
    f"validated {len(SCHEMAS)} schemas, {len(MANIFESTS)} plugin manifests "
    f"and {len(providers)} configured model providers"
)
