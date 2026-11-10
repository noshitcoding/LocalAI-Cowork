"""Actual stdio adapter probe: cwd and secret environment slots reach only the child."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest


class PersistentMcpEnvironmentTest(unittest.TestCase):
    def test_child_receives_environment_and_cwd_without_plaintext_config(self):
        adapter = pathlib.Path(__file__).resolve().parents[2] / 'crates/toolbox/src/persistent.py'
        secret = 'fixture-private-stdio-value'
        with tempfile.TemporaryDirectory(prefix='cowork-stdio-probe-') as directory:
            root = pathlib.Path(directory)
            workspace = root / 'chosen-workspace'
            workspace.mkdir()
            base_environment = {key: value for key, value in os.environ.items() if key in {'SystemRoot', 'SYSTEMROOT', 'WINDIR'}}
            base_environment['PATH'] = str(pathlib.Path(sys.executable).parent)
            script = '''import json, os, sys
for line in sys.stdin:
    request = json.loads(line)
    result = {'cwd': os.getcwd(), 'has_value': os.environ.get('API_KEY', '').startswith('fixture-'), 'empty': os.environ.get('EMPTY') == '', 'slot_hidden': not any(k.startswith('COWORK_MCP_VALUE_') for k in os.environ)}
    print(json.dumps({'jsonrpc':'2.0', 'id':request['id'], 'result':result}), flush=True)
'''
            configuration = {'mode': 'mcp', 'program': sys.executable, 'arguments': ['-I', '-u', '-c', script],
                             'workspace': str(workspace), 'environment': base_environment,
                             'environment_slots': {'API_KEY': 'COWORK_MCP_VALUE_ONE', 'EMPTY': 'COWORK_MCP_VALUE_TWO'}}
            raw = json.dumps(configuration)
            self.assertNotIn(secret, raw)
            (root / 'config.json').write_text(raw, encoding='utf-8')
            environment = dict(base_environment, COWORK_MCP_VALUE_ONE=secret, COWORK_MCP_VALUE_TWO='')
            process = subprocess.Popen([sys.executable, '-I', str(adapter), str(root)], env=environment,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
            try:
                self.wait_for(root / 'ready.json', process)
                request = {'body': {'jsonrpc': '2.0', 'id': 1, 'method': 'probe'}, 'timeout_seconds': 5}
                (root / 'probe.request.json').write_text(json.dumps(request), encoding='utf-8')
                response = root / 'probe.response.json'
                self.wait_for(response, process)
                reply = json.loads(response.read_text(encoding='utf-8'))
                self.assertNotIn(secret, json.dumps(reply))
                self.assertEqual(reply['result']['result'], {'cwd': str(workspace), 'has_value': True, 'empty': True, 'slot_hidden': True})
                self.assertNotIn(secret, (root / 'config.json').read_text(encoding='utf-8'))
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                process.stdout.close()
                process.stderr.close()

    def wait_for(self, path, process):
        deadline = time.monotonic() + 5
        while not path.is_file():
            self.assertIsNone(process.poll(), 'Adapter exited before the probe completed')
            self.assertLess(time.monotonic(), deadline, 'Adapter probe timed out')
            time.sleep(0.01)


if __name__ == '__main__':
    unittest.main()
