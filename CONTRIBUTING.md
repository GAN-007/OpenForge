# Contributing to OpenForge

OpenForge accepts focused changes that preserve the control-plane invariants: canonical daemon state, explicit capability boundaries, sandboxed autonomous execution, deterministic task scheduling, Git-native reviewability, provider neutrality and attributable events.

## Development checks

Before opening a pull request, run:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
corepack enable
pnpm install
pnpm typecheck
pnpm build
python -m pip install -e "python[dev]"
pytest python/tests
```

Changes to protocol, event, policy, task or plugin contracts must update the corresponding schema and compatibility documentation.

Security-sensitive changes require tests covering both the allowed path and at least one denied path. Do not weaken a deny rule merely to make an agent action succeed.

## Commit discipline

Use small, reviewable commits. Generated changes are acceptable only when the contributor understands and verifies them. Never commit provider keys, cloud credentials, production database credentials, private repository content or raw audit logs containing sensitive data.

## Architecture decisions

Material changes to canonical state ownership, transport protocol, security boundaries, runner authority, plugin capability semantics or licensing require an ADR in `docs/adr/`.
