# Install and launch OpenForge

From the checkout, run:

```bash
./setup.sh
```

Choose **CLI / terminal**, **IDE**, **native desktop GUI**, or **browser**. The script checks the selected interface's dependencies, installs missing ones, builds the application, starts or reuses a local daemon, then opens the interface. It does not modify your project's files or shell startup files. Builds and downloads can take several minutes on the first launch.

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

For a short command in your current shell:

```bash
export PATH="/absolute/path/to/OpenForge/target/debug:$PATH"
export OPENFORGE_DAEMON_URL=http://127.0.0.1:8875
openforge chat /absolute/project
```

## IDE and desktop

IDE mode detects VS Code, VSCodium, or Code Insiders. On Debian/Ubuntu, if none exists it installs Microsoft's official VS Code Debian package. It opens the built OpenForge extension using the editor's extension development host and an isolated `.code-workspace` file under `.openforge/runtime`. Existing project `.vscode` settings are preserved. This mode provides the extension's existing task tree, search and inline-completion features; select `openforge.runId` to view a run and enable run-scoped completion. The browser console also opens for model connection and run creation. JetBrains plugin packaging remains a separate Gradle workflow.

GUI mode installs the Linux WebKitGTK 4.1 and Tauri development dependencies, starts the desktop frontend on a free port, and launches the real Tauri development window with the matching URL/CSP. A graphical session is required. See [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for non-Debian systems. Desktop icons are generated from the existing OpenForge extension SVG.

## Services and limits

Daemon and frontend processes continue running when setup returns, so switching interfaces preserves the session. Their PIDs are printed and logs are in `.openforge/runtime/{daemon,web,desktop-web}.log`. Repeated frontend launches currently create another local frontend process; the daemon is reused. Stop only the printed PIDs you started when finished. A rebuilt binary does not update an already-running daemon: restart it deliberately to load an updated backend. Keys saved by the updated gateway are restored automatically.

The script installs the dependencies needed for local interfaces. Docker, Kubernetes credentials/PVCs, remote model access, JetBrains/Java, and optional evaluation environments are separate services/toolchains, not silently provisioned. It does not grant policy approvals or change deployment settings. Native GUI/IDE opening requires an available desktop session; remote/headless users should use terminal or `--no-open` browser mode.
