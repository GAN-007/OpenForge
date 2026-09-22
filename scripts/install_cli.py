"""Install the checkout's terminal launcher for the current Unix user."""
from pathlib import Path
import os
import shlex

ROOT = Path(__file__).resolve().parents[1]
MARKER = '# OpenForge user CLI launcher'
PATH_MARKER = '# OpenForge user CLI PATH'


def install(home: Path, root: Path = ROOT) -> Path:
    binary = root / 'target/debug/openforge'
    if not binary.is_file():
        raise RuntimeError('Build OpenForge before installing its terminal command')
    destination = home / '.local/bin/openforge'
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_symlink() or (destination.exists() and MARKER not in destination.read_text()):
        raise RuntimeError(f'Refusing to replace an unrelated command: {destination}')
    script = f'''#!/bin/sh
{MARKER}
if [ "$#" -eq 0 ]; then
    exec python3 {shlex.quote(str(root / 'scripts/launch.py'))} --interface cli --project "$PWD"
fi
if [ -z "${{OPENFORGE_DAEMON_URL:-}}" ] && [ -r {shlex.quote(str(root / '.openforge/runtime/daemon-url.txt'))} ]; then
    IFS= read -r OPENFORGE_DAEMON_URL < {shlex.quote(str(root / '.openforge/runtime/daemon-url.txt'))}
    export OPENFORGE_DAEMON_URL
fi
exec {shlex.quote(str(binary))} "$@"
'''
    destination.write_text(script)
    destination.chmod(0o755)
    block = f'''\n{PATH_MARKER}
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) export PATH="$HOME/.local/bin:$PATH" ;;
esac
'''
    for name in ('.profile', '.bashrc', '.zshrc'):
        startup = home / name
        # Do not create a Zsh configuration for users who do not use Zsh.
        if name == '.zshrc' and not startup.exists() and not os.environ.get('SHELL', '').endswith('/zsh'):
            continue
        previous = startup.read_text() if startup.exists() else ''
        if PATH_MARKER not in previous:
            with startup.open('a') as stream:
                stream.write(block)
    print(f'Installed terminal command: {destination}')
    print('Type openforge in any project folder. For this existing shell, run: export PATH="$HOME/.local/bin:$PATH"')
    return destination


if __name__ == '__main__':
    install(Path.home())
