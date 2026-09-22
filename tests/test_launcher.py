import importlib.util
import json
import io
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('launcher', ROOT / 'scripts/launch.py')
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)


class LauncherTests(unittest.TestCase):
    def test_setup_help_and_invalid_options_do_not_install(self):
        result = subprocess.run([str(ROOT / 'setup.sh'), '--help'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('--skip-install', result.stdout)
        result = subprocess.run([str(ROOT / 'setup.sh'), '--interface', 'invalid'], capture_output=True)
        self.assertEqual(result.returncode, 2)

    def test_rejects_nonlocal_or_credential_bearing_daemon_urls(self):
        for value in ['https://example.com', 'http://user:secret@localhost:8765',
                      'http://localhost:8765/path', 'http://localhost:8765?token=x']:
            with self.assertRaises(ValueError):
                launcher.local_url(value)
        self.assertEqual(launcher.local_url('http://127.0.0.1:8875/'), 'http://127.0.0.1:8875')

    def test_busy_port_is_skipped_without_terminating_owner(self):
        with socket.socket() as server:
            server.bind(('127.0.0.1', 0))
            occupied = server.getsockname()[1]
            self.assertNotEqual(launcher.free_port(occupied), occupied)
            self.assertEqual(server.getsockname()[1], occupied)

    def test_existing_daemon_is_reused_and_not_restarted(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(launcher, 'RUNTIME', Path(tmp)):
            with patch.object(launcher, 'health', side_effect=lambda url: url.endswith(':8875')):
                with patch.object(launcher, 'spawn') as spawn:
                    self.assertEqual(launcher.daemon_url(None), 'http://127.0.0.1:8875')
                    spawn.assert_not_called()

    def test_explicit_unavailable_daemon_fails_without_starting_another(self):
        with patch.object(launcher, 'health', return_value=False), patch.object(launcher, 'spawn') as spawn:
            with self.assertRaises(RuntimeError):
                launcher.daemon_url('http://127.0.0.1:8875')
            spawn.assert_not_called()

    def test_restart_refuses_active_runs_or_unverifiable_state(self):
        for payload in [{'result': [{'status': 'running'}]}, {'error': {'code': -1}}]:
            with patch.object(launcher, 'owned_daemon_pid', return_value=123):
                with patch.object(launcher.HTTP, 'open', return_value=io.StringIO(json.dumps(payload))):
                    with patch.object(launcher.os, 'kill') as kill:
                        with self.assertRaises(RuntimeError):
                            launcher.restart_daemon('http://127.0.0.1:8875')
                        kill.assert_not_called()

    def test_restart_uses_only_identified_idle_process(self):
        with patch.object(launcher, 'owned_daemon_pid', return_value=123):
            with patch.object(launcher.HTTP, 'open', return_value=io.StringIO('{"result": []}')):
                with patch.object(launcher.os, 'kill') as kill, patch.object(launcher, 'spawn') as spawn:
                    with patch.object(launcher, 'health', return_value=False), patch.object(launcher, 'wait_ready'):
                        launcher.restart_daemon('http://127.0.0.1:8875')
                    kill.assert_called_once_with(123, launcher.signal.SIGTERM)
                    self.assertIn('127.0.0.1:8875', spawn.call_args.args[0])

    def test_ide_workspace_preserves_project_and_uses_same_daemon(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(launcher, 'RUNTIME', Path(tmp)):
            project = Path(tmp) / 'project with spaces'
            project.mkdir()
            with patch.object(launcher.shutil, 'which', return_value='/usr/bin/code'):
                command = launcher.ide_command(project, 'http://127.0.0.1:8875')
            workspace = json.loads(Path(command[-1]).read_text())
            self.assertEqual(workspace['settings']['openforge.daemonUrl'], 'http://127.0.0.1:8875')
            self.assertEqual(workspace['folders'], [{'path': str(project)}])
            self.assertEqual(list(project.iterdir()), [])

    def test_setup_flushes_shell_command_cache_after_node_bootstrap(self):
        source = (ROOT / 'setup.sh').read_text()
        extraction = source.index('tar -xJf "$TOOLS/$archive"')
        cache_reset = source.index('hash -r', extraction)
        pnpm_check = source.index('if ! command -v pnpm', extraction)
        self.assertLess(cache_reset, pnpm_check)

    def test_frontend_uses_same_origin_proxy_to_selected_daemon(self):
        with patch.object(launcher, 'free_port', return_value=5191), patch.object(launcher, 'spawn') as spawn:
            with patch.object(launcher, 'wait_ready'):
                self.assertEqual(launcher.frontend('http://127.0.0.1:8875'), 'http://127.0.0.1:5191')
            env = spawn.call_args.args[2]
            self.assertEqual(env['OPENFORGE_DAEMON_URL'], 'http://127.0.0.1:8875')
            self.assertEqual(env['VITE_OPENFORGE_DAEMON_URL'], '/openforge-daemon')


if __name__ == '__main__':
    unittest.main()
