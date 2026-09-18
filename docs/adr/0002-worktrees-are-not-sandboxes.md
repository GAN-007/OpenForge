# ADR 0002: Worktrees are not sandboxes

Status: Accepted

## Decision

Git worktrees are used for concurrent task editing and branch isolation only. Autonomous execution requires an OS/container/VM/remote-runner boundary in addition to a worktree.

## Rationale

Git worktrees share repository metadata and host filesystem authority. Treating them as containment would allow untrusted generated commands or repository code to affect the host outside the intended task scope.
