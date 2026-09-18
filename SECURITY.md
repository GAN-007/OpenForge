# OpenForge Security Policy

OpenForge executes model-generated engineering actions. Security boundaries are therefore part of the core architecture, not an optional UI feature.

## Supported security boundary

The permission engine is an authorization layer. It is not a sandbox. `autonomous` mode is refused unless the caller selects an isolated runner. Docker is the first built-in isolation backend; stronger VM/microVM and remote-worker backends can implement the same runner interface.

Task worktrees isolate concurrent edits but share Git repository metadata and are not security boundaries.

## Default controls

The development policy is deny-dominant. It blocks common credential paths, writes to `.git`, SSH, `sudo`, `git push`, Terraform apply, destructive Kubernetes actions, and broad secret access unless an explicitly different policy is supplied.

Model credentials remain in the control plane/provider process. They are not copied into a code-execution sandbox by OpenForge.

## Threats considered

The project threat model includes malicious repositories, prompt injection, poisoned MCP servers and plugins, command injection, credential theft, data exfiltration, SSRF, browser attacks, sandbox escape, unsafe auto-approval, CI/CD privilege escalation, provider data leakage, malicious dependencies, cross-agent contamination and runaway spend.

See `docs/threat-model/README.md` for controls and trust boundaries.

## Reporting a vulnerability

Do not open a public issue for a vulnerability that would expose users before a fix exists. Use GitHub's private vulnerability reporting for this repository when enabled. Include affected version/commit, reproduction conditions, expected impact and the smallest safe proof required to reproduce it.

Security fixes should include a regression test and, when a boundary changed, an architecture or threat-model update.
