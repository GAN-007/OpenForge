"""Exercise real terminal input, not just piped command strings."""
import http.server
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('OPENFORGE_TEST_BINARY', ROOT / 'target/debug/openforge'))


@unittest.skipUnless(BINARY.exists(), 'build the Rust CLI first')
class TerminalPtyTests(unittest.TestCase):
    def test_slash_picker_editing_and_session_commands_do_not_call_models(self):
        calls = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def respond(self, value):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(json.dumps(value).encode())

            def do_GET(self):
                self.respond({'status': 'ok', 'protocol': 'openforge.protocol.v2'})

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                calls.append(request['method'])
                result = {'connected': True} if request['method'] == 'gateway/status' else []
                self.respond({'jsonrpc': '2.0', 'id': request['id'], 'result': result})

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                project = Path(tmp) / 'project'
                project.mkdir()
                subprocess.run(['git', 'init', '-q', str(project)], check=True)
                policy = Path(tmp) / 'policy.yaml'
                policy.write_text('autonomy: execute\n')
                master, slave = pty.openpty()
                import fcntl
                import struct
                import termios
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
                process = subprocess.Popen([str(BINARY), '--daemon-url', f'http://127.0.0.1:{server.server_port}', 'chat', str(project), '--policy', str(policy)],
                                           stdin=slave, stdout=slave, stderr=slave,
                                           env={**os.environ, 'XDG_STATE_HOME': tmp, 'TERM': 'xterm-256color'})
                os.close(slave)

                pending = b''

                def until(needle):
                    nonlocal pending
                    output = pending
                    pending = b''
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        if needle in output:
                            end = output.index(needle) + len(needle)
                            pending = output[end:]
                            return output[:end]
                        if select.select([master], [], [], .1)[0]:
                            output += os.read(master, 65536)
                    self.fail(f'Missing {needle!r} in {output[-2000:]!r}')

                try:
                    until(b'openforge>')
                    os.write(master, b'/')
                    until(b'/status')  # menu appears without Enter
                    os.write(master, b'st\r')
                    until(b'budget: USD')
                    until(b'\x1b[?2004h')
                    os.write(master, b'/budet\x1b[D\x1b[Dg\x1b[F 2\r')
                    until(b'Next-turn run budget: USD 2.00')
                    until(b'\x1b[?2004h')
                    os.write(master, b'/fork\r')
                    until(b'Forked session:')
                    until(b'\x1b[?2004h')
                    os.write(master, b'/not-a-command\r')
                    until(b'No model request was sent')
                    until(b'\x1b[?2004h')
                    os.write(master, b'\x1b[200~line1\n\tline2\x1b[201~')
                    until(b'line2')
                    os.write(master, b'\x03/quit\r')
                    process.wait(timeout=10)
                    self.assertEqual(process.returncode, 0)
                    self.assertEqual(calls, ['gateway/status', 'model/list'])
                    sessions = list((Path(tmp) / 'openforge/sessions').glob('*.json'))
                    self.assertEqual(len(sessions), 2)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
                    os.close(master)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    unittest.main()
