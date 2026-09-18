# Threat Model

OpenForge assumes that model output, repository content, web content, MCP output, plugins, package scripts and generated code can all be hostile.

## Assets

Protected assets include host credentials, source code, private prompts, production databases, cloud control planes, Git history, signing keys, model-provider credentials, CI identities and user budget.

## Trust classes

1. system policy and administrator configuration;
2. direct user intent;
3. project policy;
4. repository content;
5. external web content;
6. MCP/plugin output;
7. model-generated output.

Lower-trust content cannot grant itself higher-trust capabilities.

## Principal threats and controls

### Repository and prompt injection

Repository instructions are treated as data. Shell/filesystem/network/secrets operations require structured tool actions and policy evaluation. Autonomous execution uses an isolated runner.

### Command injection

Processes are represented as argv arrays, not shell-concatenated strings. Policy matches normalized command representations. High-risk commands are denied in the default policy.

### Filesystem escape

Agent file operations reject absolute and parent-relative paths. Existing read/write targets are canonicalized against the workspace, and non-existing write targets verify their nearest existing ancestor before creation.

### Secret theft

Provider credentials remain in the control plane. The secret-broker interface exposes named, allowlisted retrieval rather than inheriting every host environment variable. Production brokers should perform operations on behalf of agents where practical rather than returning reusable credentials.

### Network and SSRF

Network capability is separately policy-controlled. Browser navigation allows HTTP(S) only. Production browser runners should additionally enforce egress allowlists and private-address restrictions at the network layer.

### Malicious plugins and MCP servers

Plugin manifests declare filesystem/network/secret/database/shell capabilities. Process and MCP plugins should run under independent OS restrictions. Installation should pin content digests and release signatures.

### Supply-chain compromise

CI checks source, dependencies and container builds. Releases should publish SBOM/provenance, sign tags/artifacts and pin external workflow actions to reviewed revisions.

### Cross-agent contamination

Each coding task has an isolated working tree. Agents receive task-scoped context and permissions. Accepted changes enter the integration branch only after verification.

### Runaway spend and loops

Every model invocation is cost-accounted. Task hard budgets, model-call limits, tool-call limits, wall-time bounds and provider routing constraints stop uncontrolled loops.

### Incorrect merge

Task acceptance runs before commit and again after integration. Conflicts fail closed. Protected deployment/release actions remain outside the default agent policy.

## Known boundary

Container isolation reduces host exposure but is not equivalent to a hardened VM against a kernel escape. High-risk untrusted workloads should use a VM/microVM or separately trusted remote worker backend.
