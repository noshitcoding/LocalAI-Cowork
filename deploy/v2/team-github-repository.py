"""Approved, authenticated Git operations in one team's order workspace.

This adapter never reads an operator Git profile. It manages repositories cloned
by this adapter, replaces their local config with a credential-free fixed config,
and sends the ephemeral team credential only to the approved HTTPS origin.
"""
import base64
import json
import os
import pathlib
import re
import selectors
import stat
import subprocess
import sys
import time
import uuid
from urllib.parse import quote, urlsplit

GIT = '/usr/bin/git'
PYTHON = '/opt/cowork/python/bin/python'
ADAPTER = '/opt/cowork-team-tools/github_repository.py'
OPERATIONS = {'clone', 'checkout', 'fetch', 'pull', 'sync', 'status', 'push'}


def require(value, message):
    if not value:
        raise ValueError(message)


def host_name(value):
    require(isinstance(value, str) and len(value) <= 253
            and re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?', value)
            and all(label and len(label) <= 63 and not label.startswith('-')
                    and not label.endswith('-') for label in value.split('.')),
            'Invalid configured GitHub hostname')
    return value.lower()


def repository_url(value, host):
    require(isinstance(value, str) and len(value) <= 1024
            and not any(ord(c) <= 32 or ord(c) == 127 for c in value)
            and '%' not in value and '\\' not in value, 'Invalid GitHub repository URL')
    parsed = urlsplit(value)
    parts = parsed.path.removeprefix('/').split('/')
    require(parsed.scheme == 'https' and parsed.hostname == host and parsed.port is None
            and not parsed.username and not parsed.password and not parsed.query
            and not parsed.fragment and len(parts) == 2, 'Repository must use the configured HTTPS GitHub host')
    owner, name = parts[0], parts[1].removesuffix('.git')
    require(all(part not in {'.', '..'} and not part.startswith('-')
                and re.fullmatch(r'[A-Za-z0-9_.-]{1,160}', part) for part in (owner, name)),
            'Invalid GitHub repository name')
    return 'https://' + host + '/' + owner + '/' + name + '.git'


def reference_name(value):
    require(isinstance(value, str) and 1 <= len(value) <= 240
            and not value.startswith(('-', '/', '.')) and not value.endswith(('/', '.'))
            and not any(part in value for part in ('..', '//', '@{'))
            and not any(part.endswith('.lock') for part in value.split('/'))
            and re.fullmatch(r'[A-Za-z0-9_./-]+', value), 'Invalid Git reference')
    return value


def ordinary(path, directory=False):
    info = path.lstat()
    require(not stat.S_ISLNK(info.st_mode)
            and (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)),
            'Repository metadata must be ordinary files and directories')
    if not directory:
        require(info.st_nlink == 1, 'Linked repository metadata refused')
    return info


def workspace_path(root, value, clone):
    require(isinstance(value, str) and 0 < len(value.encode()) <= 4096
            and not any(ord(c) < 32 or ord(c) == 127 for c in value)
            and ':' not in value and '\\' not in value, 'Invalid workspace directory')
    parts = value.split('/')
    require(all(part and part not in {'.', '..', '.git'} for part in parts),
            'Repository directory must be relative to the order workspace')
    current = root
    for part in parts[:-1]:
        current = current / part
        ordinary(current, True)
    target = current / parts[-1]
    if clone:
        require(not target.exists() and not target.is_symlink(), 'Clone requires a new repository directory')
    else:
        ordinary(target, True)
    require(target.resolve(strict=False).is_relative_to(root), 'Repository escapes its order workspace')
    return target


def validate_gitdir(repo, expected):
    gitdir = repo / '.git'
    ordinary(gitdir, True)
    # Linked worktrees, object alternates, symlinked metadata and external object
    # databases could cross the order boundary before authentication begins.
    for name in ('commondir', 'gitdir', 'objects/info/alternates', 'objects/info/http-alternates'):
        require(not (gitdir / name).exists() and not (gitdir / name).is_symlink(),
                'External Git metadata refused')
    count = 0
    for parent, directories, files in os.walk(gitdir, followlinks=False):
        for name in directories:
            ordinary(pathlib.Path(parent) / name, True)
        for name in files:
            ordinary(pathlib.Path(parent) / name)
        count += len(directories) + len(files)
        require(count <= 100000, 'Repository metadata exceeds the bounded operation limit')
    marker = gitdir / 'cowork-repository.json'
    require(ordinary(marker).st_size <= 4096, 'Invalid managed Git repository receipt')
    value = json.loads(marker.read_text(encoding='utf-8'))
    require(value == {'format': 1, 'repository': expected},
            'Repository was not cloned for this approved GitHub URL')
    return gitdir


def write_config(gitdir, repository, empty_hooks):
    config = gitdir / 'config'
    ordinary(config)
    # Replacing the complete local config removes includes, URL rewrites, custom
    # credential helpers, remote helpers, filters, proxies and trace settings.
    data = ('[core]\n\trepositoryformatversion = 0\n\tbare = false\n'
            '\tfilemode = true\n\tlogallrefupdates = true\n'
            '\tfsmonitor = false\n\tattributesfile = /dev/null\n'
            '\texcludesfile = /dev/null\n\thookspath = ' + str(empty_hooks) + '\n'
            '\tprotectNTFS = true\n\tprotectHFS = true\n'
            '[remote "origin"]\n\turl = ' + repository + '\n'
            '\tfetch = +refs/heads/*:refs/remotes/origin/*\n'
            '[credential]\n\thelper =\n\tuseHttpPath = true\n'
            '[http]\n\tsslVerify = true\n\tfollowRedirects = false\n'
            '[submodule]\n\trecurse = false\n[fetch]\n\tfsckObjects = true\n'
            '[transfer]\n\tfsckObjects = true\n[gc]\n\tauto = 0\n'
            '[maintenance]\n\tauto = false\n')
    temporary = gitdir / ('cowork-config-' + uuid.uuid4().hex)
    with temporary.open('x', encoding='utf-8', newline='\n') as file:
        file.write(data)
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, config)


def redact(raw, token):
    candidates = [token.encode(), quote(token, safe='').encode(),
                  base64.b64encode(('x-access-token:' + token).encode()),
                  base64.b64encode((':' + token).encode())]
    for secret in sorted(set(candidates), key=len, reverse=True):
        if secret:
            raw = raw.replace(secret, b'[REDACTED]')
    return raw


def git(command, repo, environment, token, deadline, emit=True, allowed_codes=(0,)):
    child = subprocess.Popen([GIT, *command], cwd=repo, env=environment,
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    selector = selectors.DefaultSelector()
    outputs = [bytearray(), bytearray()]
    overflow = False
    for stream, index in ((child.stdout, 0), (child.stderr, 1)):
        os.set_blocking(stream.fileno(), False)
        selector.register(stream, selectors.EVENT_READ, index)
    try:
        while selector.get_map():
            require(time.monotonic() < deadline, 'Approved Git operation exceeded its timeout')
            for key, _ in selector.select(timeout=.2):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                remaining = max(0, 2 * 1024 * 1024 - sum(map(len, outputs)))
                outputs[key.data].extend(chunk[:remaining])
                overflow = overflow or len(chunk) > remaining
        code = child.wait(timeout=max(.1, deadline - time.monotonic()))
        if emit:
            for stream, raw in ((sys.stdout.buffer, outputs[0]), (sys.stderr.buffer, outputs[1])):
                stream.write(redact(bytes(raw), token))
                stream.flush()
        require(not overflow, 'Git output exceeds the bounded operation limit')
        require(code in allowed_codes, 'Approved Git command failed with exit code ' + str(code))
        return bytes(outputs[0])
    finally:
        selector.close()
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        else:
            child.wait()


def credentials(environment, token, host, repository):
    environment.update({'GIT_TERMINAL_PROMPT': '0', 'GIT_ASKPASS': PYTHON + ' -I ' + ADAPTER + ' --askpass',
                        'COWORK_GITHUB_TOKEN': token, 'COWORK_GITHUB_HOST': host,
                        'COWORK_GITHUB_REPOSITORY': repository})


def askpass():
    require(len(sys.argv) == 3, 'Invalid Git credential prompt')
    token = os.environ.get('COWORK_GITHUB_TOKEN', '')
    host = host_name(os.environ.get('COWORK_GITHUB_HOST', ''))
    expected = repository_url(os.environ.get('COWORK_GITHUB_REPOSITORY', ''), host)
    prompt = sys.argv[2]
    match = re.fullmatch(r"(Username|Password) for '([^']+)': ?", prompt)
    require(match is not None and token, 'Unexpected Git credential request')
    parsed = urlsplit(match[2])
    require(parsed.scheme == 'https' and parsed.hostname == host and parsed.port is None
            and not parsed.password and not parsed.query and not parsed.fragment
            and parsed.username in {None, 'x-access-token'}
            and parsed.path in {'', urlsplit(expected).path}, 'Foreign Git credential request refused')
    print('x-access-token' if match[1] == 'Username' else token)


def execute(action, token, configured_host, workspace):
    require(isinstance(action, dict) and set(action) <= {'operation', 'repository', 'directory', 'reference', 'depth', 'source_commit'}
            and {'operation', 'repository', 'directory'} <= set(action), 'Unknown or missing repository action fields')
    operation = action['operation']
    require(operation in OPERATIONS, 'Unknown Git repository operation')
    host = host_name(configured_host)
    require(isinstance(token, str) and 0 < len(token.encode()) <= 4096
            and not any(ord(c) <= 32 or ord(c) == 127 for c in token), 'Missing or invalid team GitHub credential')
    repository = repository_url(action['repository'], host)
    reference = reference_name(action['reference']) if action.get('reference') is not None else None
    depth = action.get('depth')
    require(depth is None or (operation == 'clone' and type(depth) is int and 1 <= depth <= 1000),
            'Depth is only available for clone and must be between 1 and 1000')
    require(operation != 'checkout' or reference is not None, 'Checkout requires an explicit revision')
    require(operation != 'status' or reference is None, 'Status does not accept a revision')
    source_commit = action.get('source_commit')
    require((operation == 'push' and reference is not None and isinstance(source_commit, str)
             and re.fullmatch(r'[0-9a-fA-F]{40}|[0-9a-fA-F]{64}', source_commit))
            or (operation != 'push' and source_commit is None),
            'Push requires an exact committed HEAD and destination branch')
    root = pathlib.Path(workspace).resolve(strict=True)
    ordinary(root, True)
    repo = workspace_path(root, action['directory'], operation == 'clone')
    control = pathlib.Path('/tmp') / ('cowork-git-' + uuid.uuid4().hex)
    control.mkdir(mode=0o700)
    empty = control / 'empty-hooks'
    empty.mkdir(mode=0o700)
    environment = {'LANG': 'C.UTF-8', 'HOME': str(control), 'TMPDIR': '/tmp',
                   'PATH': '/opt/cowork/python/bin:/usr/bin:/bin', 'GIT_CONFIG_NOSYSTEM': '1',
                   'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_SYSTEM': '/dev/null',
                   'GIT_ALLOW_PROTOCOL': 'https', 'GIT_PROTOCOL_FROM_USER': '0',
                   'GIT_TERMINAL_PROMPT': '0', 'GIT_CEILING_DIRECTORIES': str(root)}
    # These options apply to the first clone too, before a managed local config
    # exists. No workspace or parent Git config is consulted for clone.
    options = ['-c', 'core.hooksPath=' + str(empty), '-c', 'core.fsmonitor=false',
               '-c', 'credential.helper=', '-c', 'credential.useHttpPath=true',
               '-c', 'http.followRedirects=false', '-c', 'http.sslVerify=true',
               '-c', 'protocol.allow=never', '-c', 'protocol.https.allow=always',
               '-c', 'submodule.recurse=false', '-c', 'fetch.fsckObjects=true',
               '-c', 'transfer.fsckObjects=true', '-c', 'gc.auto=0', '-c', 'maintenance.auto=false']
    credentials(environment, token, host, repository)
    deadline = time.monotonic() + 100
    if operation == 'clone':
        command = ['clone', '--no-checkout', '--no-recurse-submodules', '--origin', 'origin', '--template=' + str(empty)]
        if depth is not None:
            command += ['--depth', str(depth)]
        git(options + command + ['--', repository, str(repo)], control, environment, token, deadline)
        gitdir = repo / '.git'
        ordinary(gitdir, True)
        marker = gitdir / 'cowork-repository.json'
        with marker.open('x', encoding='utf-8') as file:
            json.dump({'format': 1, 'repository': repository}, file, separators=(',', ':'))
            file.flush()
            os.fsync(file.fileno())
        validate_gitdir(repo, repository)
        write_config(gitdir, repository, empty)
        if reference is None:
            head=git(options + ['rev-parse', '--verify', '--quiet', 'HEAD^{commit}'],repo,environment,token,deadline,False,(0,1)).strip()
            if head:
                require(re.fullmatch(rb'[0-9a-f]{40,64}',head), 'Invalid cloned commit')
                git(options + ['read-tree', '--reset', '-u', 'HEAD'], repo, environment, token, deadline)
        else:
            git(options + ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', reference], repo, environment, token, deadline)
            commit = git(options + ['rev-parse', '--verify', 'FETCH_HEAD^{commit}'], repo, environment, token, deadline, False).decode().strip()
            require(re.fullmatch(r'[0-9a-f]{40,64}', commit), 'Invalid fetched commit')
            git(options + ['checkout', '--no-recurse-submodules', '--detach', commit], repo, environment, token, deadline)
    else:
        gitdir = validate_gitdir(repo, repository)
        write_config(gitdir, repository, empty)
        if operation in {'checkout', 'pull', 'sync'}:
            dirty = git(options + ['status', '--porcelain=v1', '-z', '--untracked-files=normal'], repo, environment, token, deadline, False)
            require(not dirty, 'Working tree contains changes; commit or move them before checkout or sync')
        if operation == 'checkout':
            git(options + ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', reference], repo, environment, token, deadline)
            commit = git(options + ['rev-parse', '--verify', 'FETCH_HEAD^{commit}'], repo, environment, token, deadline, False).decode().strip()
            require(re.fullmatch(r'[0-9a-f]{40,64}', commit), 'Invalid fetched commit')
            git(options + ['checkout', '--no-recurse-submodules', '--detach', commit], repo, environment, token, deadline)
        elif operation == 'fetch':
            git(options + ['fetch', '--no-tags', '--no-recurse-submodules', 'origin'] + ([reference] if reference else []), repo, environment, token, deadline)
        elif operation == 'push':
            # The approval freezes both the local commit and destination. A later
            # workspace edit cannot broaden this request; Git refuses non-fast-forward.
            head = git(options + ['rev-parse', '--verify', 'HEAD^{commit}'], repo, environment, token, deadline, False).decode().strip()
            require(head == source_commit.lower(), 'Approved commit no longer matches repository HEAD')
            require(not reference.startswith('refs/'), 'Push requires a branch name, not a refspec')
            git(options + ['push', '--porcelain', '--no-verify', '--recurse-submodules=no',
                           'origin', head + ':refs/heads/' + reference], repo, environment, token, deadline)
        elif operation in {'pull', 'sync'}:
            if reference is None:
                reference = git(options + ['symbolic-ref', '--quiet', '--short', 'HEAD'], repo, environment, token, deadline, False).decode().strip()
                reference_name(reference)
            git(options + ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', reference], repo, environment, token, deadline)
            commit = git(options + ['rev-parse', '--verify', 'FETCH_HEAD^{commit}'], repo, environment, token, deadline, False).decode().strip()
            require(re.fullmatch(r'[0-9a-f]{40,64}', commit), 'Invalid fetched commit')
            git(options + ['merge', '--ff-only', '--no-edit', '--no-stat', '--no-verify', '--no-gpg-sign', commit], repo, environment, token, deadline)
        else:
            git(options + ['status', '--short', '--branch'], repo, environment, token, deadline)
    commit = git(options + ['rev-parse', '--verify', '--quiet', 'HEAD^{commit}'], repo, environment, token, deadline, False,(0,1) if operation in {'clone','status'} else (0,)).decode().strip()
    require((not commit and operation in {'clone','status'}) or re.fullmatch(r'[0-9a-f]{40,64}', commit), 'Repository has no committed HEAD')
    receipt=json.dumps({'operation': operation, 'repository': repository, 'directory': action['directory'],
                       'commit': commit or None, 'reference': reference, 'source_commit': source_commit,
                       'force': False, 'completed': True}, separators=(',', ':'))
    sys.stdout.buffer.write(redact(receipt.encode(),token)+b'\n')
    sys.stdout.buffer.flush()


if __name__ == '__main__':
    try:
        require(len(sys.argv) > 1 and sys.argv[1] == '--askpass', 'Repository adapter must be invoked by the approved guest')
        askpass()
    except (ValueError, OSError, TypeError, KeyError):
        # A credential prompt is never echoed, including on failure.
        raise SystemExit(1)
