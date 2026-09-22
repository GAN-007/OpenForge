# Install and launch OpenForge

From the checkout, run:

```bash
./setup.sh
```

Choose **CLI / terminal**, **IDE**, **native desktop GUI**, or **browser**. The script checks the selected interface's dependencies, installs missing ones, builds the application, starts or reuses a local daemon, then opens the interface. It installs a user command at `~/.local/bin/openforge` and adds an idempotent PATH entry to Bash/login shell startup files (and existing Zsh configuration). It preserves project files. Builds and downloads can take several minutes on the first launch.

Automatic system package installation supports Debian/Ubuntu with `apt-get`; it uses `sudo` only for missing system packages or an IDE installation. Rust is installed through the official rustup installer if needed. Node 22 binaries are downloaded from nodejs.org and checked against its SHA-256 manifest; pnpm is pinned to the workspace version. Downloaded tools live under `.openforge/tools`, not in the system Node installation. Existing compatible tools are reused. Other Unix systems can provision the reported tools with their package manager and use `--skip-install`. Native Windows is not supported by this Bash bootstrap.

```bash
./setup.sh --check --interface gui
./setup.sh --interface browser --project /absolute/project
./setup.sh --interface cli --project /absolute/project
./setup.sh --interface ide --project /absolute/project
./setup.sh --interface gui
./setup.sh --install-only --interface browser
./setup.sh --skip-install --skip-build --interface browser --no-open
```

`--check` only reports dependencies. `--skip-install` still builds unless paired with `--skip-build`; both flags require existing artifacts. `--install-only` installs/builds without starting services. `--no-open` suppresses browser opening and prints its URL. Use `--daemon-url http://127.0.0.1:PORT` to explicitly select a running daemon; an unavailable explicit endpoint fails instead of silently selecting another provider.

## Shared model connection

All launcher interfaces use the same daemon. The launcher checks for an existing OpenForge protocol endpoint on ports 8875 and 8765, preferring its saved endpoint, before starting one. An unrelated service is never stopped. Web frontends use an available loopback port from 5180 and a same-origin Vite proxy, so occupied ports do not require changing your daemon's CORS configuration.

Enter your Sevi key in **Model connection → Sevi model gateway → Test & connect**, or enter `/connect` in the terminal. CLI planning, execution and completion then use the same gateway as the GUI. The alias `auto-select` can select different underlying models per request. Successfully tested keys are saved by the daemon in its private per-user configuration file and restored on restart; they never enter launcher state. Use **Disconnect & forget key** to remove the saved credential. See [credential storage details](providers/sevi.md). Sevi controls access, quotas and charges.

`OPENFORGE_API_TOKEN` is the separate daemon authentication token. The CLI reads it from the environment; authenticated browser/desktop clients require it in their daemon-token field, and the IDE uses **OpenForge: Set API Token**. The launcher does not copy either credential into workspace files.

## Terminal

The Rust CLI now uses the daemon by default. The previous direct-engine mode remains available with `--standalone` and uses the provider configuration in `openforge.yaml`.

```bash
./target/debug/openforge --daemon-url http://127.0.0.1:8875 chat /absolute/project
./target/debug/openforge --daemon-url http://127.0.0.1:8875 plan "Review failing tests" /absolute/project
./target/debug/openforge --daemon-url http://127.0.0.1:8875 gateway status
./target/debug/openforge --standalone providers
```

Chat accepts objectives and keeps bounded recent session context. Each objective creates and plans a new run; execute mode runs its tasks and reports an integration branch. It does not automatically merge that branch into your checkout. `/help`, `/gateway`, `/provider`, `/connect`, `/disconnect`, `/context`, `/clear`, and `/quit` are supported. Use `chat --mode suggest` to plan without execution. The normal policy still governs tools and commands; this is an objective-driven terminal, not a full-screen TUI or unrestricted filesystem shell.

Git workspaces are required for execution. For a non-Git folder, chat asks before creating local Git metadata and a baseline commit. That baseline stages the folder's non-ignored files; review `.gitignore` first. No initialization is performed by the launcher itself.

After setup, open a new terminal (or run `export PATH="$HOME/.local/bin:$PATH"` in the existing terminal). From any project folder:

```bash
openforge
openforge --help
```

With no arguments, `openforge` starts interactive chat for the current directory and starts or reuses the shared daemon. Arguments are forwarded to the Rust CLI unchanged. The launcher references this checkout, so rerun setup if it is moved. Setup refuses to overwrite an unrelated existing `~/.local/bin/openforge` command.

`/files` (also `list all files and folders`) lists the current project's files locally without model calls or creating a run. It respects ignore rules and excludes Git metadata. Failed objectives report an error and leave the terminal open for the next command.

To load a newly built backend, use `./setup.sh --interface cli --restart-daemon`. On Linux this checks the daemon executable, checkout, port and run state before stopping it. It refuses to restart unidentified services or daemons with running tasks; saved gateway credentials are restored by the replacement daemon.

## IDE and desktop

IDE mode detects VS Code, VSCodium, or Code Insiders. On Debian/Ubuntu, if none exists it installs Microsoft's official VS Code Debian package. It opens the built OpenForge extension using the editor's extension development host and an isolated `.code-workspace` file under `.openforge/runtime`. Existing project `.vscode` settings are preserved. This mode provides the extension's existing task tree, search and inline-completion features; select `openforge.runId` to view a run and enable run-scoped completion. The browser console also opens for model connection and run creation. JetBrains plugin packaging remains a separate Gradle workflow.

GUI mode installs the Linux WebKitGTK 4.1 and Tauri development dependencies, starts the desktop frontend on a free port, and launches the real Tauri development window with the matching URL/CSP. A graphical session is required. See [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for non-Debian systems. Desktop icons are generated from the existing OpenForge extension SVG.

## Services and limits

Daemon and frontend processes continue running when setup returns, so switching interfaces preserves the session. Their PIDs are printed and logs are in `.openforge/runtime/{daemon,web,desktop-web}.log`. Repeated frontend launches currently create another local frontend process; the daemon is reused. Stop only the printed PIDs you started when finished. A rebuilt binary does not update an already-running daemon: restart it deliberately to load an updated backend. Keys saved by the updated gateway are restored automatically.

The script installs the dependencies needed for local interfaces. Docker, Kubernetes credentials/PVCs, remote model access, JetBrains/Java, and optional evaluation environments are separate services/toolchains, not silently provisioned. It does not grant policy approvals or change deployment settings. Native GUI/IDE opening requires an available desktop session; remote/headless users should use terminal or `--no-open` browser mode.

## Automation

See [GitHub and MCP extensions](automation.md) for connecting GitHub, discovering tools and adding other automation servers.

Local terminal requests `list all files in this folder`, `ls`, `/files`, `whoami`, and `pwd` do not create runs or call a model. Listing aliases use OpenForge's ignore-aware recursive listing, not shell `ls` formatting. Arbitrary shell syntax is not executed by these aliases.

For model-driven objectives, planner cost estimates determine proportional shares of the remaining user-selected run budget after planning costs. A single task can use that remaining budget instead of being capped by an arbitrarily small model estimate. The run's dollar limit and reported provider charges still apply; Sevi is not guaranteed to be free.
