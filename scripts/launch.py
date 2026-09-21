"""Local interface launcher. Never stores provider credentials or edits project settings."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
from urllib.error import URLError
from urllib.parse import urlparse
from urllib.request import ProxyHandler, Request, build_opener
import webbrowser

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ROOT / '.openforge' / 'runtime'
HTTP = build_opener(ProxyHandler({}))


def local_url(value: str) -> str:
    parsed = urlparse(value)
    if (parsed.scheme != 'http' or parsed.hostname not in ('127.0.0.1', 'localhost')
            or parsed.username or parsed.password or parsed.path not in ('', '/')
            or parsed.query or parsed.fragment):
        raise ValueError('Daemon URL must be a local http://127.0.0.1:PORT address')
    return value.rstrip('/')


def health(url: str) -> bool:
    try:
        with HTTP.open(url + '/health', timeout=1) as response:
            body = json.load(response)
        return body.get('status') == 'ok' and body.get('protocol') == 'openforge.protocol.v2'
    except (OSError, ValueError, AttributeError):
        return False


def free_port(first: int) -> int:
    for port in range(first, first + 100):
        with socket.socket() as sock:
            try:
                sock.bind(('127.0.0.1', port))
                return port
            except OSError:
                continue
    raise RuntimeError(f'No free local port near {first}')


def spawn(command: list[str], name: str, environment: dict[str, str]) -> subprocess.Popen:
    RUNTIME.mkdir(parents=True, exist_ok=True)
    with (RUNTIME / f'{name}.log').open('ab') as log:
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    print(f'{name} started (PID {process.pid}); log: {RUNTIME / (name + ".log")}', flush=True)
    return process


def wait_ready(process: subprocess.Popen, ready, name: str) -> None:
    for _ in range(100):
        if process.poll() is not None:
            raise RuntimeError(f'{name} exited; inspect {RUNTIME / (name + ".log")}')
        if ready():
            return
        time.sleep(.2)
    process.terminate()
    raise RuntimeError(f'{name} startup timed out; inspect {RUNTIME / (name + ".log")}')


def daemon_url(explicit: str | None) -> str:
    if explicit:
        url = local_url(explicit)
        if not health(url):
            raise RuntimeError(f'No compatible OpenForge daemon at {url}')
        return url
    saved = RUNTIME / 'daemon-url.txt'
    candidates = []
    if saved.exists():
        candidates.append(local_url(saved.read_text().strip()))
    candidates += ['http://127.0.0.1:8875', 'http://127.0.0.1:8765']
    for url in candidates:
        if health(url):
            print(f'Reusing OpenForge daemon at {url}; active model connection is preserved.')
            return url
    port = free_port(8875)
    url = f'http://127.0.0.1:{port}'
    process = spawn([str(ROOT / 'target/debug/openforge-daemon'), '--config',
                     str(ROOT / 'openforge.yaml'), '--listen', f'127.0.0.1:{port}'],
                    'daemon', os.environ.copy())
    wait_ready(process, lambda: health(url), 'daemon')
    saved.write_text(url + '\n')
    return url


def frontend(daemon: str, desktop: bool = False) -> str:
    package = '@openforge/desktop' if desktop else '@openforge/web-console'
    name = 'desktop-web' if desktop else 'web'
    environment = os.environ.copy()
    environment['OPENFORGE_DAEMON_URL'] = daemon
    environment['VITE_OPENFORGE_DAEMON_URL'] = '/openforge-daemon'
    port = free_port(5180)
    url = f'http://127.0.0.1:{port}'
    process = spawn(['pnpm', '--filter', package, 'dev', '--host', '127.0.0.1',
                     '--port', str(port), '--strictPort'], name, environment)
    wait_ready(process, lambda: health(url + '/openforge-daemon'), name)
    return url


def open_browser(url: str, no_open: bool) -> None:
    print(f'OpenForge: {url}', flush=True)
    print('Model connection → Sevi model gateway → paste key → Test & connect.', flush=True)
    if not no_open:
        if sys.platform.startswith('linux') and not (os.getenv('DISPLAY') or os.getenv('WAYLAND_DISPLAY')):
            print('No graphical display detected. Open the URL from a browser on this machine.')
        elif not webbrowser.open(url):
            print('Could not open a browser automatically; use the URL above.')


def ide_command(project: Path, daemon: str) -> list[str]:
    executable = next((shutil.which(name) for name in ('code', 'codium', 'code-insiders')
                       if shutil.which(name)), None)
    if not executable:
        raise RuntimeError('VS Code/VSCodium is missing; rerun setup without --skip-install')
    workspace = RUNTIME / 'openforge.code-workspace'
    workspace.write_text(json.dumps({'folders': [{'path': str(project)}],
                                    'settings': {'openforge.daemonUrl': daemon}}, indent=2))
    return [executable, '--new-window',
            f'--extensionDevelopmentPath={ROOT / "packages/vscode-extension"}', str(workspace)]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--interface', choices=['cli', 'ide', 'gui', 'browser'], required=True)
    parser.add_argument('--project', type=Path, default=Path.cwd())
    parser.add_argument('--daemon-url')
    parser.add_argument('--no-open', action='store_true')
    args = parser.parse_args()
    project = args.project.resolve(strict=True)
    if not project.is_dir():
        raise ValueError('Project must be a directory')
    RUNTIME.mkdir(parents=True, exist_ok=True)
    daemon = daemon_url(args.daemon_url)
    print(f'Shared daemon: {daemon}', flush=True)
    print('Background services remain running after this launcher exits.', flush=True)
    if args.interface == 'cli':
        executable = ROOT / 'target/debug/openforge'
        print('Use /help for terminal commands. Each objective creates a run; edits are on integration branches.', flush=True)
        os.execv(str(executable), [str(executable), '--daemon-url', daemon, 'chat', str(project),
                                  '--policy', str(ROOT / 'config/policies/development.yaml')])
    elif args.interface == 'ide':
        command = ide_command(project, daemon)
        subprocess.run(command, check=True)
        print('IDE opened with the OpenForge extension. Set openforge.runId for tasks/completions.')
        print('If daemon authentication is enabled, use OpenForge: Set API Token in the IDE.')
        open_browser(frontend(daemon), args.no_open)
    elif args.interface == 'gui':
        url = frontend(daemon, desktop=True)
        config = RUNTIME / 'tauri-launch.json'
        config.write_text(json.dumps({'build': {'beforeDevCommand': '', 'devUrl': url},
                                     'app': {'security': {'csp': "default-src 'self'; connect-src 'self' "
                                                          + url + ' ' + url.replace('http:', 'ws:')
                                                          + "; style-src 'self' 'unsafe-inline'"}}}))
        environment = os.environ.copy()
        environment['VITE_OPENFORGE_DAEMON_URL'] = '/openforge-daemon'
        subprocess.run(['pnpm', '--filter', '@openforge/desktop', 'tauri', 'dev',
                        '--config', str(config)], cwd=ROOT, env=environment, check=True)
    else:
        open_browser(frontend(daemon), args.no_open)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, URLError) as error:
        print(f'OpenForge launcher: {error}', file=sys.stderr)
        sys.exit(1)
