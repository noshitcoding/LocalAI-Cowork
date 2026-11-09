"""Run the real Linux Secret Service test inside a fresh, private DBus session."""
import os
import pathlib
import secrets
import subprocess
import sys
import tempfile

assert sys.platform == 'linux' and os.getuid() != 0
assert os.environ.get('DBUS_SESSION_BUS_ADDRESS'), 'invoke using dbus-run-session'
with tempfile.TemporaryDirectory(prefix='cowork-v2-secret-service-') as temp:
    env = os.environ.copy()
    env['HOME'] = temp
    env['XDG_DATA_HOME'] = str(pathlib.Path(temp) / 'data')
    env['XDG_CONFIG_HOME'] = str(pathlib.Path(temp) / 'config')
    env['XDG_CACHE_HOME'] = str(pathlib.Path(temp) / 'cache')
    env['XDG_RUNTIME_DIR'] = str(pathlib.Path(temp) / 'runtime')
    pathlib.Path(env['XDG_RUNTIME_DIR']).mkdir(mode=0o700)
    # Only this throwaway keyring receives this generated test password.
    result = subprocess.run(['gnome-keyring-daemon', '--unlock', '--components=secrets'],
        env=env, input=secrets.token_hex(32)+'\n', capture_output=True, text=True, timeout=20)
    if result.returncode:
        raise SystemExit('Synthetic Secret Service could not start')
    # CARGO_HOME/RUSTUP_HOME must remain explicitly set by the test image.
    for package, suite, case in [
        ('cowork-secrets', 'native', 'native_key_survives_worker_termination_without_plaintext_fallback'),
        ('cowork-runtime', 'native_setup_process', 'native_setup_process_receipt_and_uncertainty_survive_hard_kill'),
    ]:
        command = ['cargo', 'test', '--locked', '--offline', '-p', package, '--test', suite,
            case, '--', '--ignored', '--exact']
        code = subprocess.run(command, env=env).returncode
        if code:
            raise SystemExit(code)
