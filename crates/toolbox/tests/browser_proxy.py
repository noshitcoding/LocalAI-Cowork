"""Check proxy delivery at both browser launch boundaries without network I/O."""
import contextlib
import io
import json
import os
import pathlib
import runpy
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'src' / 'browser.py'


class BrowserProxyTests(unittest.TestCase):
    def run_browser(self, environment, modules, popen=None):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            executable = root / 'chromium'
            executable.touch()
            arguments = [str(SCRIPT), str(root), str(root), str(executable), json.dumps({
                'operation': 'dom', 'url': 'https://example.org', 'session_id': 'proxy-test',
            })]
            with patch.dict(os.environ, environment, clear=True), patch.dict(sys.modules, modules), \
                    patch.object(sys, 'argv', arguments), contextlib.redirect_stdout(io.StringIO()):
                if popen is None:
                    runpy.run_path(str(SCRIPT), run_name='__main__')
                else:
                    with patch('subprocess.Popen', popen):
                        runpy.run_path(str(SCRIPT), run_name='__main__')

    def test_playwright_proxy_precedence_and_unset(self):
        cases = [
            ({'HTTPS_PROXY': 'http://secure-proxy:3128', 'HTTP_PROXY': 'http://web-proxy:3128',
              'ALL_PROXY': 'http://fallback-proxy:3128'}, 'http://secure-proxy:3128'),
            ({'HTTPS_PROXY': '', 'HTTP_PROXY': 'http://web-proxy:3128'}, 'http://web-proxy:3128'),
            ({'ALL_PROXY': 'http://fallback-proxy:3128'}, 'http://fallback-proxy:3128'),
            ({}, None),
        ]
        for environment, expected in cases:
            with self.subTest(environment=environment):
                launches = []
                page = types.SimpleNamespace(url='https://example.org',
                    goto=lambda *args, **kwargs: None, evaluate=lambda *args: '<html/>', title=lambda: 'Test')
                context = types.SimpleNamespace(pages=[page], close=lambda: None)

                def launch(profile, **settings):
                    launches.append(settings)
                    return context

                playwright = types.SimpleNamespace(chromium=types.SimpleNamespace(launch_persistent_context=launch))
                module = types.ModuleType('playwright.sync_api')
                module.sync_playwright = lambda: contextlib.nullcontext(playwright)
                self.run_browser(environment, {'playwright': types.ModuleType('playwright'), 'playwright.sync_api': module})
                self.assertEqual(len(launches), 1)
                self.assertEqual(launches[0].get('proxy'), {'server': expected} if expected else None)

    def test_cli_receives_one_proxy_argument(self):
        launches = []

        def popen(command, **settings):
            launches.append(command)
            settings['stdout'].write(b'<html/>')
            return types.SimpleNamespace(wait=lambda **kwargs: 0)

        self.run_browser({'HTTPS_PROXY': 'http://egress-proxy:3128'},
            {'playwright': None, 'playwright.sync_api': None}, popen)
        self.assertEqual([value for value in launches[0] if value.startswith('--proxy-server=')],
            ['--proxy-server=http://egress-proxy:3128'])

    def test_proxy_launch_failure_is_not_retried_without_proxy(self):
        def launch(*args, **settings):
            raise ConnectionError('proxy unavailable')

        playwright = types.SimpleNamespace(chromium=types.SimpleNamespace(launch_persistent_context=launch))
        module = types.ModuleType('playwright.sync_api')
        module.sync_playwright = lambda: contextlib.nullcontext(playwright)
        with patch('subprocess.Popen') as fallback:
            with self.assertRaisesRegex(ConnectionError, 'proxy unavailable'):
                self.run_browser({'HTTPS_PROXY': 'http://egress-proxy:3128'},
                    {'playwright': types.ModuleType('playwright'), 'playwright.sync_api': module})
            fallback.assert_not_called()


if __name__ == '__main__':
    unittest.main()
