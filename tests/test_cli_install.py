import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('installer', ROOT / 'scripts/install_cli.py')
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class CliInstallTests(unittest.TestCase):
    def test_arguments_cwd_and_idempotent_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / 'home'
            root = Path(tmp) / "checkout with ' spaces"
            binary = root / 'target/debug/openforge'
            binary.parent.mkdir(parents=True)
            binary.write_text('#!/bin/sh\nprintf "%s\\n" "$PWD" "$@"\n')
            binary.chmod(0o755)
            (root / 'scripts').mkdir()
            (root / 'scripts/launch.py').write_text('import sys; print("\\n".join(sys.argv[1:]))')
            path = installer.install(home, root)
            installer.install(home, root)
            self.assertEqual((home / '.bashrc').read_text().count(installer.PATH_MARKER), 1)
            result = subprocess.run([str(path), 'plan', 'an objective'], cwd=home, capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout.splitlines(), [str(home), 'plan', 'an objective'])
            result = subprocess.run([str(path)], cwd=home, capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout.splitlines(), ['--interface', 'cli', '--project', str(home)])
            path.write_text('unrelated command')
            with self.assertRaises(RuntimeError):
                installer.install(home, root)
            self.assertEqual(path.read_text(), 'unrelated command')
