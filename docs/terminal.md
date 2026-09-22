# Interactive terminal

Run `openforge` in a project, or select CLI in `setup.sh`. Type `/` to open the command picker immediately. Type a prefix to filter, use Up/Down to select, Enter to run, Tab to complete a command before entering its arguments, and Escape to close the picker. Outside the picker, Up/Down recalls session input. Left/Right, Home/End, Backspace/Delete edit the prompt; Ctrl-U clears it, Ctrl-C clears a draft, and Ctrl-D exits an empty prompt. Bracketed paste preserves embedded newlines, tabs, and code indentation instead of flattening pasted code into one line. Piped input remains supported without terminal escape sequences.

| Command | Implemented behavior |
| --- | --- |
| `/`, `/help` | Discover available commands locally |
| `/status` | Session ID, workspace, mode, budget, policy and last run |
| `/model` | Inspect the shared daemon's model catalog; Sevi selects its upstream model |
| `/permissions [path]`, `/approvals [path]` | Inspect policy or select a validated policy file for future turns |
| `/plan`, `/mode suggest\|edit\|execute` | Change whether subsequent objectives are planned or executed |
| `/budget [USD]` | Inspect or change the next objective's run budget |
| `/init` | Create AGENTS.md without overwriting existing instructions; terminal objectives include it |
| `/review` | Model-assisted review of tracked workspace/last-run changes with writes, commands and MCP denied |
| `/diff` | Show tracked changes in the workspace and the last run's integration branch |
| `/new`, `/fork` | Save the current conversation and start a new or copied session |
| `/resume [UUID]` | List saved sessions for this workspace or restore conversation and last-run context |
| `/compact` | Explicitly truncate context to six recent messages; no model call |
| `/context`, `/clear` | Inspect or clear conversation context |
| `/runs`, `/tasks`, `/events` | Inspect recent runs or the current session's last run |
| `/mcp [server]` | List configured extensions or discover a server's tools under the active policy |
| `/apps` | List installed plugin manifests |
| `/connect`, `/disconnect`, `/gateway`, `/provider` | Manage the existing shared model connection |
| `/files`, `/pwd`, `/whoami` | Local workspace and identity inspection |
| `/quit`, `/exit` | Save and leave the terminal |

Unknown slash commands and invalid arguments to local slash commands never create a model request. During execution the terminal polls audit events and shows task/model/tool activity; completed task summaries are displayed and retained as follow-up context. Generated changes remain on the reported integration branch, where they can be inspected before merging.

Sessions are saved atomically under `$XDG_STATE_HOME/openforge/sessions` or `~/.local/state/openforge/sessions`, with Unix directory mode 0700 and file mode 0600. They contain conversation text and run IDs; gateway keys entered through the non-echoing connection prompt are not included. Resume is restricted to the same workspace and retains the current mode, budget and permissions rather than silently restoring broader access.

This interface follows familiar [Codex slash-command conventions](https://developers.openai.com/codex/cli/slash-commands) but is not full Codex feature parity. The Sevi `auto-select` gateway does not expose model/reasoning selection here. Policy `ask` decisions still stop a task; this does not implement an approve-once dialog or automatic task resumption. Progress is audit-event polling, not token streaming. Ctrl-C during active execution is not a daemon cancellation API; closing the client can leave a run active. Remote cloud jobs, voice, image prompts, and Codex-specific account features are not offered as nonfunctional menu entries.
