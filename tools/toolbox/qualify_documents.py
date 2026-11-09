"""Run mandatory document qualification with a private, explicitly provisioned TeX runtime.

Use --install-tex in CI to download the pinned binary and warm its own cache. No
user data, global installation, browser or model provider is used.
"""
import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--install-tex', action='store_true')
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[2]
    env = dict(os.environ, COWORK_REQUIRE_DOCUMENT_TESTS='1', COWORK_REQUIRE_TEX_TESTS='1',
               COWORK_TEST_DOCUMENT_PYTHON=sys.executable, COWORK_ENGINE_PYTHON=sys.executable)
    with tempfile.TemporaryDirectory(prefix='cowork-document-qualification-') as temporary:
        if args.install_tex:
            data = pathlib.Path(temporary)
            subprocess.run([sys.executable, '-I', str(root / 'tools/toolbox/runtime_install.py'),
                            '--data-dir', str(data), '--tectonic-only'], cwd=root, check=True, timeout=3600)
            config = json.loads((data / 'toolbox.json').read_text(encoding='utf-8'))
            binary = config['programs']['tectonic']
            cache = str(data / 'tool-runtime/tectonic-cache')
        else:
            binary, cache = env.get('COWORK_TECTONIC'), env.get('COWORK_TECTONIC_CACHE')
            if not binary or not cache:
                parser.error('pass --install-tex or configure COWORK_TECTONIC and COWORK_TECTONIC_CACHE')
            # The confinement suite changes Windows ACLs. Never change an existing runtime.
            directory = pathlib.Path(temporary)
            private_binary = directory / 'bin' / pathlib.Path(binary).name
            private_binary.parent.mkdir()
            shutil.copy2(binary, private_binary)
            private_cache = directory / 'cache'
            shutil.copytree(cache, private_cache)
            binary, cache = str(private_binary), str(private_cache)
        env.update(COWORK_TECTONIC=binary, COWORK_TECTONIC_CACHE=cache,
                   COWORK_TEST_TECTONIC=binary, COWORK_TEST_TECTONIC_CACHE=cache)
        commands = [
            [sys.executable, '-I', 'tools/toolbox/test_runtime_install.py'],
            [sys.executable, 'crates/toolbox/tests/document_engine/run_tests.py', '-v'],
            ['cargo', 'test', '--locked', '-p', 'cowork-domain', '-p', 'cowork-policy'],
            ['cargo', 'test', '--locked', '-p', 'cowork-broker', '--test', 'generated'],
            ['cargo', 'test', '--locked', '-p', 'cowork-storage', '--test', 'document_drafts',
             'document_drafts_keep_conflicts_and_replay_the_original_save'],
            ['cargo', 'test', '--locked', '-p', 'cowork-runtime', '--test', 'documents', '--', '--nocapture'],
            ['cargo', 'test', '--locked', '-p', 'cowork-runtime', '--test', 'workflow_documents',
             '--', '--include-ignored', '--test-threads=1', '--nocapture'],
            ['cargo', 'test', '--locked', '-p', 'cowork-executor', '--test', 'tex_confinement', '--', '--nocapture'],
            ['cargo', 'build', '--locked', '-p', 'cowork-daemon'],
            [sys.executable, '-I', 'tests/system/documents.py', '--latex'],
        ]
        for command in commands:
            subprocess.run(command, cwd=root, env=env, check=True)


if __name__ == '__main__':
    main()
