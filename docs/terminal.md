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
| `/goal [text\|edit text\|pause\|resume\|clear]` | Persist a goal across turns and inject it into future objectives until paused or cleared |
| `/personality [friendly\|pragmatic\|none]` | Persist the terminal response style used for future objectives |
| `/mention [path\|clear]` | Attach validated UTF-8 workspace files (up to 256 KiB each) to future objectives |
| `/init` | Create AGENTS.md without overwriting existing instructions; terminal objectives include it |
| `/review` | Model-assisted review of tracked workspace/last-run changes with writes, commands and MCP denied |
| `/diff` | Show tracked changes in the workspace and the last run's integration branch |
| `/new`, `/fork` | Save the current conversation and start a new or copied session |
| `/rename TITLE` | Give the current saved session a persistent title |
| `/archive`, `/delete` | Archive or delete the current session and immediately start a new private session |
| `/resume [UUID]` | List saved sessions for this workspace or restore conversation and last-run context |
| `/compact` | Explicitly truncate context to six recent messages; no model call |
| `/copy` | Copy the latest OpenForge output through the OSC 52 terminal clipboard protocol |
| `/context`, `/clear` | Inspect or clear conversation context |
| `/runs`, `/tasks`, `/events` | Inspect recent runs or the current session's last run |
| `/usage` | Show persisted reservation and spend limits for the current session's last run |
| `/debug-config` | Show daemon capabilities, gateway/model state, and the active validated policy |
| `/mcp [server\|verbose]` | List MCP servers, discover one server's tools, or enumerate tools for all configured servers under the active policy |
| `/memories [query\|all]` | Inspect repository-scoped OpenForge memory without creating a model request |
| `/skills [filter]` | Discover `SKILL.md` files in the workspace using ignore-aware traversal |
| `/apps`, `/plugins` | List installed OpenForge plugin manifests |
| `/agent`, `/subagents` | Inspect task agents from the last run plus active ACP agent processes |
| `/ps`, `/stop UUID\|all` | List or terminate live ACP agent processes |
| `/connect`, `/disconnect`, `/logout`, `/gateway`, `/provider` | Manage or inspect the existing shared Sevi/model connection |
| `/files`, `/pwd`, `/whoami` | Local workspace and identity inspection |
| `/quit`, `/exit` | Save and leave the terminal |

Unknown slash commands and invalid arguments to local slash commands never create a model request. `/plan OBJECTIVE` is an inline planning turn: it switches to suggest mode and sends only `OBJECTIVE`, not the slash command itself. During execution the terminal polls audit events and shows task/model/tool activity; completed task summaries are displayed and retained as follow-up context. Generated changes remain on the reported integration branch, where they can be inspected before merging.

Sessions are saved atomically under `$XDG_STATE_HOME/openforge/sessions` or `~/.local/state/openforge/sessions`, with Unix directory mode 0700 and file mode 0600. They contain conversation text and run IDs; gateway keys entered through the non-echoing connection prompt are not included. Resume is restricted to the same workspace and retains the current mode, budget and permissions rather than silently restoring broader access.

This interface follows familiar [Codex slash-command conventions](https://developers.openai.com/codex/cli/slash-commands), while binding commands to OpenForge's own daemon, policy, MCP/ACP, memory, plugin, budget, and session primitives. The Sevi `auto-select` gateway does not currently expose a per-session model/reasoning override, so `/model` is an inspector rather than a fake selector. Policy `ask` decisions still stop a task; OpenForge does not yet have an approve-once/resume RPC, so `/approve` is intentionally not advertised. Progress is audit-event polling rather than token streaming, and the daemon does not yet expose true run cancellation or queued input injection while a run is active. Codex account/cloud-only operations are likewise omitted instead of being presented as nonfunctional controls.

Alt-Enter or Shift-Enter inserts a line break when the terminal reports the modifier. Bracketed paste retains code indentation and line breaks; submitting a multiline prompt renders each line at the left margin.
