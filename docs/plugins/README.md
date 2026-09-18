# Plugin System

A plugin must declare authority before execution.

Manifest schema: `openforge.plugin/v1`.

Supported runtime classes are:

- `wasm`: preferred for strongly capability-limited extensions;
- `process`: external process over a narrow protocol boundary;
- `mcp`: MCP server integration.

Manifest capabilities cover filesystem, network, secrets, database and shell authority. A plugin declaration is not itself an authorization grant: policy and runtime isolation must still approve the capability.

Built-in plugins live under `plugins/builtin/`. Third-party installation should verify publisher identity, version, SHA-256 digest, signature, dependency metadata and license before activation.
