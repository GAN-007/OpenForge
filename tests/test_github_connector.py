import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('github_connector', ROOT / 'plugins/builtin/github/server.py')
connector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(connector)


class GitHubConnectorTests(unittest.TestCase):
    def test_protocol_discovery_without_credentials(self):
        requests = [dict(jsonrpc='2.0', id=1, method='initialize'),
                    dict(jsonrpc='2.0', method='notifications/initialized'),
                    dict(jsonrpc='2.0', id=2, method='tools/list')]
        result = subprocess.run(['python3', str(ROOT / 'plugins/builtin/github/server.py')],
                                input='\n'.join(map(json.dumps, requests)) + '\n', text=True,
                                capture_output=True, check=True, timeout=5)
        responses = list(map(json.loads, result.stdout.splitlines()))
        self.assertEqual(len(responses), 2)
        self.assertEqual(len(responses[1]['result']['tools']), 7)

    def test_read_and_write_use_fixed_host_and_stdin_without_shell(self):
        with patch.object(connector.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '{}', '')) as run:
            connector.invoke('list_issues', {'owner': 'GAN-007', 'repo': 'OpenForge', 'page': 2})
            self.assertIn('/repos/GAN-007/OpenForge/issues?per_page=30&page=2', run.call_args.args[0])
            self.assertIn('github.com', run.call_args.args[0])
            self.assertNotIn('shell', run.call_args.kwargs)
            connector.invoke('create_issue', {'owner': 'GAN-007', 'repo': 'OpenForge', 'title': 'a title', 'body': 'literal `text` $HOME'})
            self.assertEqual(json.loads(run.call_args.kwargs['input'])['body'], 'literal `text` $HOME')
            self.assertIn('POST', run.call_args.args[0])

    def test_invalid_arguments_cannot_escape_endpoint_or_execute_commands(self):
        for values in [{'owner': '../evil', 'repo': 'x'}, {'owner': 'x', 'repo': 'x', 'page': True},
                       {'owner': 'x', 'repo': 'x', 'host': 'evil.example'}]:
            with patch.object(connector.subprocess, 'run') as run:
                with self.assertRaises(ValueError):
                    connector.invoke('list_issues', values)
                run.assert_not_called()

    def test_upstream_errors_do_not_expose_credentials(self):
        with patch.object(connector.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', 'private-token')):
            result = connector.handle({'method': 'tools/call', 'params': {'name': 'get_repository', 'arguments': {'owner': 'x', 'repo': 'y'}}})
            self.assertTrue(result['isError'])
            self.assertNotIn('private-token', json.dumps(result))
