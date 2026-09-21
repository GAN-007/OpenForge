#!/usr/bin/env bash
# Bootstrap only the dependencies required for the selected local interface.
set -Eeuo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="$PWD"
INTERFACE=""; INSTALL=1; BUILD=1; OPEN=1; CHECK=0; INSTALL_ONLY=0
DAEMON_URL="${OPENFORGE_DAEMON_URL:-}"
usage() {
  cat <<'HELP'
Usage: ./setup.sh [options]
  --interface cli|ide|gui|browser  Skip the interface menu
  --project PATH                 Project to open (default: calling directory)
  --daemon-url URL               Reuse this local daemon; fail if unavailable
  --check                        Report dependencies without installing or launching
  --skip-install                 Use existing dependencies
  --skip-build                   Use existing compiled outputs
  --install-only                 Install/build without launching
  --no-open                      Print browser URL without opening a window
  --help                         Show this help

Automatic system installation supports Debian/Ubuntu (sudo may be required).
Other Unix systems can use --skip-install after installing the reported tools.
CLI, IDE, desktop GUI and browser share the same running daemon and model key.
HELP
}
while (($#)); do
  case "$1" in
    --interface|--project|--daemon-url)
      (($# >= 2)) || { echo "Missing value for $1" >&2; exit 2; }
      case "$1" in --interface) INTERFACE="$2";; --project) PROJECT="$2";; --daemon-url) DAEMON_URL="$2";; esac
      shift 2;;
    --check) CHECK=1; shift;;
    --skip-install) INSTALL=0; shift;;
    --skip-build) BUILD=0; shift;;
    --install-only) INSTALL_ONLY=1; shift;;
    --no-open) OPEN=0; shift;;
    --help|-h) usage; exit 0;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2;;
  esac
done
if [[ -z "$INTERFACE" ]]; then
  if ((CHECK)); then INTERFACE=browser
  elif [[ -t 0 ]]; then
    printf '\nOpenForge interface\n  1) CLI / terminal\n  2) IDE (VS Code / VSCodium)\n  3) Native desktop GUI\n  4) Browser\n'
    read -r -p 'Select [1-4, default 4]: ' choice
    case "${choice:-4}" in 1) INTERFACE=cli;; 2) INTERFACE=ide;; 3) INTERFACE=gui;; 4) INTERFACE=browser;; *) echo 'Invalid selection' >&2; exit 2;; esac
  else echo 'Use --interface cli|ide|gui|browser in non-interactive sessions.' >&2; exit 2
  fi
fi
case "$INTERFACE" in cli|ide|gui|browser) ;; *) echo "Unknown interface: $INTERFACE" >&2; exit 2;; esac
[[ -d "$PROJECT" ]] || { echo "Project directory does not exist: $PROJECT" >&2; exit 2; }
PROJECT="$(cd -- "$PROJECT" && pwd)"
TOOLS="$ROOT/.openforge/tools"
# Reuse the workspace toolchain when one has already been provisioned.
if [[ -z "${CARGO_HOME:-}" && -x "$TOOLS/cargo/bin/cargo" ]]; then
  export CARGO_HOME="$TOOLS/cargo" RUSTUP_HOME="$TOOLS/rustup"
elif [[ -z "${CARGO_HOME:-}" && -x "$ROOT/.venv/cargo/bin/cargo" ]]; then
  export CARGO_HOME="$ROOT/.venv/cargo" RUSTUP_HOME="$ROOT/.venv/rustup"
fi
export PATH="$TOOLS/node/bin:$TOOLS/pnpm/bin:${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
node_ok() { command -v node >/dev/null && node -e 'const [a,b]=process.versions.node.split(".").map(Number);process.exit(a>=22 && (a!==22 || b>=12)?0:1)' >/dev/null 2>&1; }
rust_ok() { command -v rustc >/dev/null && rustc --version | awk '{split($2,v,".");exit !(v[1]>1 || (v[1]==1 && v[2]>=88))}'; }
if ((CHECK)); then
  for tool in git curl python3 cc pkg-config cargo rustc node pnpm; do
    if command -v "$tool" >/dev/null; then printf '%-12s %s\n' "$tool" "$(command -v "$tool")"; else printf '%-12s MISSING\n' "$tool"; fi
  done
  node_ok && echo 'Node >=22.12: OK' || echo 'Node >=22.12: REQUIRED'
  rust_ok && echo 'Rust >=1.88: OK' || echo 'Rust >=1.88: REQUIRED'
  if [[ "$INTERFACE" == gui ]]; then pkg-config --exists webkit2gtk-4.1 2>/dev/null && echo 'WebKitGTK 4.1: OK' || echo 'WebKitGTK 4.1 development libraries: REQUIRED on Linux'; fi
  exit 0
fi
trap 'echo "Setup failed at line $LINENO. Resolve the reported error and rerun ./setup.sh; existing project files are preserved." >&2' ERR
mkdir -p "$TOOLS" "$ROOT/.openforge/runtime"
if ((INSTALL)); then
  if command -v apt-get >/dev/null && command -v dpkg-query >/dev/null; then
    packages=(build-essential pkg-config libssl-dev git curl ca-certificates python3 xz-utils)
    if [[ "$INTERFACE" == gui ]]; then packages+=(libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev file); fi
    if [[ "$INTERFACE" != cli ]]; then packages+=(xdg-utils); fi
    missing=()
    for package in "${packages[@]}"; do
      # Some distributions provide these commands through equivalent packages.
      if [[ "$package" == pkg-config ]] && command -v pkg-config >/dev/null; then continue; fi
      if [[ "$package" == build-essential ]] && command -v cc >/dev/null && command -v c++ >/dev/null && command -v make >/dev/null; then continue; fi
      [[ "$(dpkg-query -W -f='${Status}' "$package" 2>/dev/null || true)" == 'install ok installed' ]] || missing+=("$package")
    done
    SUDO=(); if ((EUID != 0)); then SUDO=(sudo); fi
    if ((${#missing[@]})); then
      printf 'Installing system dependencies: %s\n' "${missing[*]}"
      "${SUDO[@]}" apt-get update
      "${SUDO[@]}" apt-get install -y -- "${missing[@]}"
    fi
  else
    echo 'Automatic system packages require Debian/Ubuntu. Checking your existing Unix tools.'
  fi
  for tool in git curl python3 cc pkg-config; do command -v "$tool" >/dev/null || { echo "Install $tool using your system package manager." >&2; exit 1; }; done
  if ! rust_ok; then
    export CARGO_HOME="${CARGO_HOME:-$TOOLS/cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$TOOLS/rustup}"
    export PATH="$CARGO_HOME/bin:$PATH"
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$TOOLS/rustup-init.sh"
    sh "$TOOLS/rustup-init.sh" -y --no-modify-path --profile minimal --default-toolchain 1.88.0
  fi
  if ! node_ok; then
    case "$(uname -s)" in Linux) node_os=linux;; Darwin) node_os=darwin;; *) echo 'Install Node.js >=22.12 for this OS.' >&2; exit 1;; esac
    case "$(uname -m)" in x86_64|amd64) node_arch=x64;; aarch64|arm64) node_arch=arm64;; *) echo 'Unsupported Node binary architecture.' >&2; exit 1;; esac
    curl --proto '=https' --tlsv1.2 -fsSL https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt -o "$TOOLS/SHASUMS256.txt"
    archive="$(awk -v suffix="-$node_os-$node_arch.tar.xz" 'substr($2,length($2)-length(suffix)+1)==suffix {print $2}' "$TOOLS/SHASUMS256.txt")"
    [[ "$archive" == node-v22.* && "$archive" != *$'\n'* && "$archive" != */* ]] || { echo 'Invalid Node download manifest' >&2; exit 1; }
    curl --proto '=https' --tlsv1.2 -fsSL "https://nodejs.org/dist/latest-v22.x/$archive" -o "$TOOLS/$archive"
    python3 - "$TOOLS" "$archive" <<'PY'
import hashlib, pathlib, sys
root, name = pathlib.Path(sys.argv[1]), sys.argv[2]
expected = dict((line.split()[1], line.split()[0]) for line in (root / 'SHASUMS256.txt').read_text().splitlines())[name]
with (root / name).open('rb') as stream:
    digest = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b''): digest.update(chunk)
if digest.hexdigest() != expected: raise SystemExit('Node archive checksum mismatch')
PY
    mkdir -p "$TOOLS/node"
    tar -xJf "$TOOLS/$archive" -C "$TOOLS/node" --strip-components=1
  fi
  if ! command -v pnpm >/dev/null || [[ "$(pnpm --version)" != 10.15.1 ]]; then
    npm install --prefix "$TOOLS/pnpm" --global pnpm@10.15.1
    hash -r
  fi
  if [[ "$INTERFACE" == ide ]] && ! command -v code >/dev/null && ! command -v codium >/dev/null && ! command -v code-insiders >/dev/null; then
    command -v apt-get >/dev/null || { echo 'Install VS Code or VSCodium and make code/codium available on PATH.' >&2; exit 1; }
    case "$(uname -m)" in x86_64) code_arch=x64;; aarch64|arm64) code_arch=arm64;; *) echo 'Install a supported IDE manually.' >&2; exit 1;; esac
    curl --proto '=https' --tlsv1.2 -fsSL "https://update.code.visualstudio.com/latest/linux-deb-$code_arch/stable" -o "$TOOLS/openforge-vscode.deb"
    "${SUDO[@]}" apt-get install -y "$TOOLS/openforge-vscode.deb"
  fi
fi
for tool in git python3 cargo node pnpm; do command -v "$tool" >/dev/null || { echo "Missing $tool; rerun without --skip-install." >&2; exit 1; }; done
node_ok || { echo 'Node >=22.12 is required.' >&2; exit 1; }
rust_ok || { echo 'Rust >=1.88 is required.' >&2; exit 1; }
python3 -c 'import sys; assert sys.version_info >= (3,10), "Python >=3.10 required"'
cd -- "$ROOT"
export CARGO_TARGET_DIR="$ROOT/target"
if ((BUILD)); then
  pnpm install --frozen-lockfile
  cargo build --locked -p openforge -p openforge-daemon
  pnpm build
fi
((INSTALL_ONLY)) && { echo 'OpenForge dependencies and builds are ready.'; exit 0; }
args=(--interface "$INTERFACE" --project "$PROJECT")
[[ -z "$DAEMON_URL" ]] || args+=(--daemon-url "$DAEMON_URL")
((OPEN)) || args+=(--no-open)
exec python3 "$ROOT/scripts/launch.py" "${args[@]}"
