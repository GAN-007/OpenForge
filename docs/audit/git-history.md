# Git history snapshot — 20 September 2026

Baseline `main`: `eeb89e1` (159 reachable commits at audit start). Remotes were refreshed with `git fetch origin --prune`. GitHub default branch is `main`; Issues are enabled and the issue query returned an empty array. This describes visible history, not an assertion that every historical push or deleted branch is recoverable. Git records commits and refs, not a complete push audit trail.

Main first-parent history consolidates the initial repository, production implementation, PR #12 rebuild, PR #15 runtime integrations and PR #16 manual hardening. Recent main work added Kubernetes execution, ACP processes, secret lease records, budget reservations and streaming artifacts. Main CI/Security last observed successful at `eeb89e1`; Normalize Source failed because its unquoted condition contained a colon, now corrected.

## All visible remote branches

Ahead/behind counts are relative to the audit baseline. A branch with zero ahead commits is already contained in main. Divergence is not permission to discard main's fixes.

| Branch | SHA | Ahead | Behind | Disposition |
| --- | --- | ---: | ---: | --- |
| `build/deepseek-session-enhancement-v2` | `c68d5d7` | 0 | 38 | Already contained; no merge required |
| `build/market-ready-openforge-v1` | `e35c11e` | 122 | 37 | Diverged; reconcile and test before merge |
| `build/production-openforge-v1` | `99b6523` | 0 | 155 | Already contained; no merge required |
| `build/runtime-integration-normalize-20260919` | `55302e7` | 10 | 37 | Diverged; reconcile and test before merge |
| `dependabot/cargo/reqwest-0.13` | `4fd01a3` | 1 | 37 | Independent dependency compatibility review |
| `dependabot/cargo/rusqlite-0.40` | `9dcf41e` | 1 | 3 | Independent dependency compatibility review |
| `dependabot/cargo/sha2-0.11` | `a1c2ba0` | 1 | 3 | Independent dependency compatibility review |
| `dependabot/cargo/tower-http-0.7` | `ec21851` | 1 | 37 | Independent dependency compatibility review |
| `dependabot/gradle/jetbrains/org.jetbrains.intellij.platform-2.19.0` | `d487b3e` | 1 | 154 | Independent dependency compatibility review |
| `dependabot/gradle/jetbrains/org.jetbrains.kotlin.jvm-2.4.20` | `340b75e` | 1 | 154 | Independent dependency compatibility review |
| `dependabot/npm_and_yarn/types/node-26.5.1` | `3240321` | 1 | 154 | Independent dependency compatibility review |
| `dependabot/npm_and_yarn/typescript-7.0.2` | `f2c20f5` | 1 | 37 | Independent dependency compatibility review |
| `dependabot/npm_and_yarn/vite-8.3.0` | `045901a` | 1 | 37 | Independent dependency compatibility review |
| `dependabot/npm_and_yarn/vitejs/plugin-react-6.1.1` | `fc49a47` | 1 | 37 | Independent dependency compatibility review |
| `feat/runtime-integration-completion-20260919` | `847eaff` | 12 | 37 | Diverged; reconcile and test before merge |
| `feat/runtime-rpc-integration-v3` | `34f02a3` | 0 | 4 | Already contained; no merge required |
| `fix/runtime-manual-finalization-20260919` | `319bb19` | 0 | 1 | Already contained; no merge required |
| `main` | `eeb89e1` | 0 | 0 | Baseline |
| `rebuild/deepseek-session-2026-09-18` | `3df817d` | 0 | 154 | Already contained; no merge required |

## Pull requests

GitHub API snapshot before this change was published.

| PR | State | Branch | Title |
| --- | --- | --- | --- |
| [#16](https://github.com/GAN-007/OpenForge/pull/16) | MERGED | `fix/runtime-manual-finalization-20260919` | Finalize runtime integration manual hardening |
| [#15](https://github.com/GAN-007/OpenForge/pull/15) | MERGED | `feat/runtime-rpc-integration-v3` | Integrate runtime RPC subsystems and Kubernetes execution |
| [#14](https://github.com/GAN-007/OpenForge/pull/14) | CLOSED | `feat/runtime-integration-completion-20260919` | Complete OpenForge runtime integrations and execution backends |
| [#13](https://github.com/GAN-007/OpenForge/pull/13) | OPEN | `build/market-ready-openforge-v1` | Build OpenForge 0.3 market-ready AI IDE and distributed engineering platform |
| [#12](https://github.com/GAN-007/OpenForge/pull/12) | MERGED | `build/deepseek-session-enhancement-v2` | Rebuild OpenForge from DeepSeek session architecture |
| [#11](https://github.com/GAN-007/OpenForge/pull/11) | OPEN | `dependabot/gradle/jetbrains/org.jetbrains.kotlin.jvm-2.4.20` | chore(deps): bump org.jetbrains.kotlin.jvm from 2.1.10 to 2.4.20 in /jetbrains |
| [#10](https://github.com/GAN-007/OpenForge/pull/10) | OPEN | `dependabot/cargo/rusqlite-0.40` | build(deps): update rusqlite requirement from 0.32 to 0.40 |
| [#9](https://github.com/GAN-007/OpenForge/pull/9) | OPEN | `dependabot/npm_and_yarn/types/node-26.5.1` | chore(deps-dev): bump @types/node from 24.13.5 to 26.5.1 |
| [#8](https://github.com/GAN-007/OpenForge/pull/8) | OPEN | `dependabot/cargo/reqwest-0.13` | build(deps): update reqwest requirement from 0.12 to 0.13 |
| [#7](https://github.com/GAN-007/OpenForge/pull/7) | OPEN | `dependabot/npm_and_yarn/vitejs/plugin-react-6.1.1` | build(deps): bump @vitejs/plugin-react from 5.2.0 to 6.1.1 |
| [#6](https://github.com/GAN-007/OpenForge/pull/6) | OPEN | `dependabot/npm_and_yarn/vite-8.3.0` | build(deps): bump vite from 7.3.6 to 8.3.0 |
| [#5](https://github.com/GAN-007/OpenForge/pull/5) | OPEN | `dependabot/cargo/tower-http-0.7` | build(deps): update tower-http requirement from 0.6 to 0.7 |
| [#4](https://github.com/GAN-007/OpenForge/pull/4) | OPEN | `dependabot/npm_and_yarn/typescript-7.0.2` | build(deps): bump typescript from 5.9.3 to 7.0.2 |
| [#3](https://github.com/GAN-007/OpenForge/pull/3) | OPEN | `dependabot/cargo/sha2-0.11` | build(deps): update sha2 requirement from 0.10 to 0.11 |
| [#2](https://github.com/GAN-007/OpenForge/pull/2) | OPEN | `dependabot/gradle/jetbrains/org.jetbrains.intellij.platform-2.19.0` | chore(deps): bump org.jetbrains.intellij.platform from 2.3.0 to 2.19.0 in /jetbrains |
| [#1](https://github.com/GAN-007/OpenForge/pull/1) | MERGED | `build/production-openforge-v1` | Build OpenForge AI engineering operating system |

## Merge assessment

PR #13 is 122 commits ahead and 37 behind the baseline and touches 126 paths in a tip-to-tip comparison. It introduces agent profiles, workers, collaboration/team state, knowledge/memory, LSP/DAP/debugger/devtools, terminals, registries, Azure/Vertex adapters, and a substantially expanded desktop. Main separately added and hardened runtime integrations. A tip replacement would remove some current runtime/schema changes. This audit did not validate every line of that separate 22,000-line feature implementation; merge it only as a dedicated reconciliation effort with the runtime regression suite.

PR #14 is closed without a merge and overlaps work delivered by PRs #15/#16. Do not reopen/merge it as an independent missing feature set. PRs #2–#11 are dependency upgrades, including major-version changes: resolve/test one ecosystem at a time. Already merged branches do not require another merge.

The fixes accompanying this report are based on main and should be merged as their own tested change. No force-push or blanket merge of unrelated branches is needed.
