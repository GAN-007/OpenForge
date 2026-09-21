#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DAEMON_HOST="${OPENFORGE_DAEMON_HOST:-127.0.0.1}"
DAEMON_PORT="${OPENFORGE_DAEMON_PORT:-8765}"
DAEMON_URL="${OPENFORGE_DAEMON_URL:-http://${DAEMON_HOST}:${DAEMON_PORT}}"
RUST_TOOLCHAIN="${OPENFORGE_RUST_TOOLCHAIN:-1.88.0}"
NODE_MAJOR="${OPENFORGE_NODE_MAJOR:-22}"
PNPM_VERSION="${OPENFORGE_PNPM_VERSION:-10.15.1}"
NVM_VERSION="${OPENFORGE_NVM_VERSION:-v0.40.3}"
INTERFACE=""
WORKSPACE=""
NON_INTERACTIVE=0
SKIP_INSTALL=0

log() { printf '\033[1;34m[OpenForge]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[OpenForge]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m[OpenForge]\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<'EOF'
OpenForge setup and launcher

Usage:
  ./setup.sh [options]

Options:
  --interface cli|ide|desktop|browser
  --workspace PATH
  --non-interactive
  --skip-install       Do not install missing system/toolchain dependencies
  --help

Environment:
  OPENFORGE_DAEMON_URL          Default http://127.0.0.1:8765
  OPENFORGE_API_TOKEN           Optional bearer token shared by all clients
  OPENFORGE_MODEL_SETTINGS_PATH Optional persistent model settings path
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --interface) INTERFACE="${2:-}"; shift 2 ;;
    --workspace) WORKSPACE="${2:-}"; shift 2 ;;
    --non-interactive) NON_INTERACTIVE=1; shift ;;
    --skip-install) SKIP_INSTALL=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) die "Unknown argument: $1" ;;
  esac
done

case "${INTERFACE:-}" in
  ""|cli|ide|desktop|browser) ;;
  *) die "--interface must be cli, ide, desktop, or browser" ;;
esac

SUDO=""
if [[ "$(id -u)" -ne 0 ]] && have sudo; then
  SUDO="sudo"
fi

install_base_packages() {
  [[ "$SKIP_INSTALL" -eq 1 ]] && return 0

  case "$(uname -s)" in
    Linux)
      if have apt-get; then
        log "Installing base build dependencies with apt"
        $SUDO apt-get update
        $SUDO apt-get install -y           build-essential ca-certificates curl file git pkg-config           python3 python3-pip python3-venv wget libssl-dev
      elif have dnf; then
        log "Installing base build dependencies with dnf"
        $SUDO dnf install -y           @development-tools ca-certificates curl file git openssl-devel           pkgconf-pkg-config python3 python3-pip wget
      elif have pacman; then
        log "Installing base build dependencies with pacman"
        $SUDO pacman -Sy --needed --noconfirm           base-devel ca-certificates curl file git openssl pkgconf python python-pip wget
      else
        warn "Unsupported Linux package manager; continuing with already installed tools"
      fi
      ;;
    Darwin)
      if ! have xcode-select || ! xcode-select -p >/dev/null 2>&1; then
        die "Xcode Command Line Tools are required. Run: xcode-select --install"
      fi
      if ! have git && have brew; then brew install git; fi
      if ! have python3 && have brew; then brew install python@3.12; fi
      ;;
    *)
      warn "Automatic package installation is supported on Linux and macOS; checking existing tools"
      ;;
  esac
}

install_desktop_packages() {
  [[ "$SKIP_INSTALL" -eq 1 ]] && return 0
  [[ "$(uname -s)" != "Linux" ]] && return 0

  if have apt-get; then
    log "Installing Tauri desktop dependencies"
    $SUDO apt-get update
    $SUDO apt-get install -y       libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev       libgtk-3-dev libxdo-dev patchelf
  elif have dnf; then
    $SUDO dnf install -y       webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel       librsvg2-devel libxdo-devel patchelf
  elif have pacman; then
    $SUDO pacman -Sy --needed --noconfirm       webkit2gtk-4.1 gtk3 libappindicator-gtk3 librsvg xdotool patchelf
  fi
}

ensure_rust() {
  if ! have rustup; then
    [[ "$SKIP_INSTALL" -eq 1 ]] && die "rustup is missing"
    log "Installing rustup"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
      sh -s -- -y --profile minimal
    # shellcheck disable=SC1090
    source "$HOME/.cargo/env"
  fi
  log "Installing Rust ${RUST_TOOLCHAIN} with rustfmt and clippy"
  rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal --component rustfmt --component clippy
  rustup override set "$RUST_TOOLCHAIN" --path "$ROOT"
}

ensure_node() {
  local current_major=0
  if have node; then
    current_major="$(node --version | sed -E 's/^v([0-9]+).*/\1/')"
  fi

  if [[ "$current_major" -lt "$NODE_MAJOR" ]]; then
    [[ "$SKIP_INSTALL" -eq 1 ]] && die "Node ${NODE_MAJOR}+ is required"
    log "Installing Node ${NODE_MAJOR} through nvm"
    export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"
    if [[ ! -s "$NVM_DIR/nvm.sh" ]]; then
      curl -fsSL "https://raw.githubusercontent.com/nvm-sh/nvm/${NVM_VERSION}/install.sh" | bash
    fi
    # shellcheck disable=SC1090
    source "$NVM_DIR/nvm.sh"
    nvm install "$NODE_MAJOR"
    nvm use "$NODE_MAJOR"
  fi

  if ! have corepack; then
    [[ "$SKIP_INSTALL" -eq 1 ]] && die "corepack is missing"
    npm install --global corepack
  fi
  corepack enable || true
  corepack prepare "pnpm@${PNPM_VERSION}" --activate
}

ensure_python() {
  have python3 || die "python3 is required"
  python3 - <<'PY'
import sys
if sys.version_info < (3, 10):
    raise SystemExit("Python 3.10+ is required")
PY
}

install_user_binaries() {
  mkdir -p "$HOME/.local/bin"
  ln -sfn "$ROOT/target/release/openforge" "$HOME/.local/bin/openforge"
  ln -sfn "$ROOT/target/release/openforge-daemon" "$HOME/.local/bin/openforge-daemon"
  export PATH="$HOME/.local/bin:$PATH"

  local shell_rc=""
  case "${SHELL:-}" in
    */zsh) shell_rc="$HOME/.zshrc" ;;
    */bash) shell_rc="$HOME/.bashrc" ;;
  esac
  if [[ -n "$shell_rc" ]] && ! grep -Fq 'HOME/.local/bin' "$shell_rc" 2>/dev/null; then
    printf '\n# OpenForge user binaries\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$shell_rc"
  fi
}

build_openforge() {
  log "Building Rust CLI and daemon"
  (
    cd "$ROOT"
    cargo fmt --all --check
    cargo build --release -p openforge -p openforge-daemon
  )

  log "Installing and validating TypeScript workspace"
  (
    cd "$ROOT"
    pnpm install --no-frozen-lockfile
    pnpm typecheck
    pnpm build
  )

  log "Validating Python SDK in an isolated virtual environment"
  (
    cd "$ROOT"
    python3 -m venv .openforge/python-venv
    . .openforge/python-venv/bin/activate
    python -m pip install --upgrade pip >/dev/null
    python -m pip install -e "python[dev]" >/dev/null
    python -m pytest python/tests
  )
}

daemon_online() {
  curl -fsS --max-time 2 "$DAEMON_URL/health" >/dev/null 2>&1
}

start_daemon() {
  if daemon_online; then
    log "Reusing running daemon at $DAEMON_URL"
    return 0
  fi

  mkdir -p "$ROOT/.openforge"
  local daemon_bin="$ROOT/target/release/openforge-daemon"
  [[ -x "$daemon_bin" ]] || die "Daemon binary is missing after build"

  log "Starting shared OpenForge daemon at $DAEMON_URL"
  (
    cd "$ROOT"
    nohup "$daemon_bin"       --config "$ROOT/openforge.yaml"       --listen "${DAEMON_HOST}:${DAEMON_PORT}"       >"$ROOT/.openforge/daemon.log" 2>&1 &
    echo $! >"$ROOT/.openforge/daemon.pid"
  )

  for _ in {1..40}; do
    if daemon_online; then
      log "Daemon is ready"
      return 0
    fi
    sleep 0.25
  done

  tail -n 80 "$ROOT/.openforge/daemon.log" >&2 || true
  die "Daemon failed to become healthy"
}

choose_workspace() {
  if [[ -n "$WORKSPACE" ]]; then
    WORKSPACE="$(cd "$WORKSPACE" && pwd)"
    return
  fi
  if [[ "$NON_INTERACTIVE" -eq 1 ]]; then
    WORKSPACE="$(pwd)"
    return
  fi

  local answer=""
  read -r -p "Workspace/project path [$(pwd)]: " answer
  WORKSPACE="${answer:-$(pwd)}"
  WORKSPACE="$(cd "$WORKSPACE" && pwd)"
}

choose_interface() {
  [[ -n "$INTERFACE" ]] && return
  if [[ "$NON_INTERACTIVE" -eq 1 ]]; then
    INTERFACE="cli"
    return
  fi

  printf '\nChoose how to use OpenForge:\n'
  printf '  1) CLI / interactive terminal\n'
  printf '  2) IDE (VS Code or VSCodium)\n'
  printf '  3) Desktop GUI (Tauri)\n'
  printf '  4) Browser web console\n'
  local choice=""
  read -r -p "Selection [1]: " choice
  case "${choice:-1}" in
    1) INTERFACE="cli" ;;
    2) INTERFACE="ide" ;;
    3) INTERFACE="desktop" ;;
    4) INTERFACE="browser" ;;
    *) die "Invalid interface selection" ;;
  esac
}

open_url() {
  local url="$1"
  if have xdg-open; then
    xdg-open "$url" >/dev/null 2>&1 &
  elif have open; then
    open "$url"
  else
    warn "Open this URL manually: $url"
  fi
}

launch_cli() {
  log "Launching interactive terminal against the shared daemon"
  exec "$ROOT/target/release/openforge"     --daemon-url "$DAEMON_URL"     chat "$WORKSPACE"
}

install_code_if_possible() {
  if have code || have codium; then return 0; fi
  [[ "$SKIP_INSTALL" -eq 1 ]] && return 1

  if have snap; then
    log "Installing Visual Studio Code through snap"
    $SUDO snap install code --classic
  fi
  have code || have codium
}

launch_ide() {
  local editor=""
  if have code; then editor="code"; fi
  if [[ -z "$editor" ]] && have codium; then editor="codium"; fi
  if [[ -z "$editor" ]] && install_code_if_possible; then
    if have code; then editor="code"; else editor="codium"; fi
  fi
  [[ -n "$editor" ]] || die "VS Code or VSCodium is required for IDE mode"

  (
    cd "$ROOT"
    pnpm --filter openforge-vscode build
  )
  log "Launching OpenForge IDE extension in $editor"
  exec "$editor"     --extensionDevelopmentPath="$ROOT/packages/vscode-extension"     "$WORKSPACE"
}

launch_desktop() {
  install_desktop_packages
  log "Launching OpenForge desktop GUI"
  cd "$ROOT"
  exec pnpm --filter @openforge/desktop tauri dev
}

launch_browser() {
  mkdir -p "$ROOT/.openforge"
  if ! curl -fsS --max-time 1 http://127.0.0.1:5173 >/dev/null 2>&1; then
    log "Starting OpenForge browser console"
    (
      cd "$ROOT"
      nohup pnpm --filter @openforge/web-console dev         --host 127.0.0.1 --port 5173         >"$ROOT/.openforge/web-console.log" 2>&1 &
      echo $! >"$ROOT/.openforge/web-console.pid"
    )
  fi
  for _ in {1..40}; do
    if curl -fsS --max-time 1 http://127.0.0.1:5173 >/dev/null 2>&1; then
      open_url http://127.0.0.1:5173
      log "Browser console: http://127.0.0.1:5173"
      return 0
    fi
    sleep 0.25
  done
  tail -n 80 "$ROOT/.openforge/web-console.log" >&2 || true
  die "Browser console failed to start"
}

install_base_packages
have git || die "git is required"
have curl || die "curl is required"
ensure_rust
ensure_node
ensure_python
build_openforge
install_user_binaries
start_daemon
choose_workspace
choose_interface

case "$INTERFACE" in
  cli) launch_cli ;;
  ide) launch_ide ;;
  desktop) launch_desktop ;;
  browser) launch_browser ;;
esac
