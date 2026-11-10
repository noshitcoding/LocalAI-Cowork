"""Trusted PID-1 Git supervisor. The tenant has no upstream network or credentials."""
import base64, hashlib, http.server, json, os, pathlib, selectors, signal, stat, subprocess, sys, threading, time, uuid

ROOT = pathlib.Path('/workspace')
LIMIT = 16 * 1024 * 1024
META_LIMIT = 8 * 1024 * 1024

def require(value):
    if not value: raise ValueError('invalid_private_github_operation')

def pairs(values):
    result = {}
    for key, value in values:
        require(key not in result); result[key] = value
    return result

def parse(raw): return json.loads(raw, object_pairs_hook=pairs)

def line(limit=12 * 1024 * 1024):
    result = bytearray()
    while True:
        b = os.read(0, 1); require(b)
        if b == b'\n': return parse(result)
        require(len(result) < limit); result.extend(b)

def emit(value):
    raw = json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode()
    require(len(raw) <= 32 * 1024 * 1024)
    sys.stdout.buffer.write(raw + b'\n'); sys.stdout.buffer.flush()

def relative(raw):
    require(isinstance(raw, str) and raw and len(raw.encode()) <= 2000)
    require('\\' not in raw and ':' not in raw and not any(ord(c) < 32 for c in raw))
    require(all(p and p not in {'.', '..'} for p in raw.split('/')))
    return pathlib.Path(*raw.split('/'))

def regular_tree(root, limit, skip_git=False):
    values, names, total = [], set(), 0
    for path in sorted(root.rglob('*')):
        name = path.relative_to(root).as_posix(); relative(name)
        if skip_git and any(p.casefold() == '.git' for p in name.split('/')): continue
        info = path.lstat(); require(not stat.S_ISLNK(info.st_mode) and name.casefold() not in names)
        names.add(name.casefold()); require(len(names) <= 10000)
        if stat.S_ISDIR(info.st_mode): values.append({'path': name, 'kind': 'directory'})
        else:
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1)
            with open(path, 'rb') as file: raw = file.read(limit + 1)
            require(len(raw) == info.st_size); total += len(raw); require(total <= limit)
            values.append({'path': name, 'kind': 'file', 'data': base64.b64encode(raw).decode()})
    return values, total

def stop_children(primary=None):
    require(os.getpid() == 1)
    try: os.kill(-1, signal.SIGKILL)
    except ProcessLookupError: pass
    if primary is not None: primary.wait(timeout=5)
    deadline = time.monotonic() + 5
    while True:
        try:
            while os.waitpid(-1, os.WNOHANG)[0]: pass
        except ChildProcessError: pass
        live = []
        for entry in pathlib.Path('/proc').iterdir():
            if entry.name.isdigit() and int(entry.name) != 1:
                try:
                    if (entry/'stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z': live.append(entry.name)
                except FileNotFoundError: pass
        if not live: return
        require(time.monotonic() < deadline); time.sleep(.01)

def env():
    return {'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'HOME': '/tmp', 'TMPDIR': '/tmp',
            'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_TERMINAL_PROMPT': '0',
            'GIT_ASKPASS': '/bin/false', 'GIT_SSH_COMMAND': '/bin/false', 'GIT_PROTOCOL_FROM_USER': '0',
            'GIT_OPTIONAL_LOCKS': '0', 'GIT_NO_REPLACE_OBJECTS': '1'}

def command(directory, arguments):
    # Every argument is produced here from a typed operation, never a model shell string.
    return ['/usr/bin/git', '-c', 'core.hooksPath=/dev/null', '-c', 'credential.helper=',
            '-c', 'protocol.allow=never', '-c', 'protocol.http.allow=always', '-c', 'http.followRedirects=false',
            '-c', 'http.proxy=', '-c', 'http.extraHeader=', '-c', 'http.version=HTTP/1.1',
            '-c', 'http.maxRequests=1', '-c', 'gc.auto=0', '-c', 'core.fsmonitor=false',
            '-c', 'core.attributesFile=/dev/null', '-c', 'diff.external=', '-C', str(directory), *arguments]

def git(directory, arguments):
    child = subprocess.Popen(command(directory, arguments), env=env(), stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, user=1000, group=1000, extra_groups=[],
        close_fds=True, start_new_session=True)
    selector = selectors.DefaultSelector(); outputs = {'stdout': bytearray(), 'stderr': bytearray()}; total = {'stdout': 0, 'stderr': 0}
    for stream, name in ((child.stdout, 'stdout'), (child.stderr, 'stderr')):
        os.set_blocking(stream.fileno(), False); selector.register(stream, selectors.EVENT_READ, name)
    start = time.monotonic()
    while selector.get_map():
        if time.monotonic() - start > 90: child.kill()
        for key, _ in selector.select(.05):
            raw = os.read(key.fileobj.fileno(), 8192)
            if not raw: selector.unregister(key.fileobj); key.fileobj.close(); continue
            total[key.data] += len(raw); outputs[key.data].extend(raw[:max(0, 1024 * 1024 - len(outputs[key.data]))])
        if time.monotonic() - start > 100: raise ValueError('private_git_timeout')
    code = child.wait(timeout=5)
    return {'exit_code': code, 'stdout': outputs['stdout'].decode('utf-8', errors='replace'),
            'stderr': outputs['stderr'].decode('utf-8', errors='replace'), 'stdout_bytes': total['stdout'],
            'stderr_bytes': total['stderr'], 'output_truncated': any(total[k] > len(outputs[k]) for k in total)}

def succeed(directory, arguments):
    result = git(directory, arguments); require(result['exit_code'] == 0 and not result['output_truncated'])
    return result['stdout'].strip()

def head(directory):
    result = git(directory, ['rev-parse', '--verify', 'HEAD'])
    if result['exit_code'] == 0: return result['stdout'].strip()
    require(succeed(directory, ['symbolic-ref', 'HEAD']).startswith('refs/heads/'))
    return '0' * 40

def metadata_name(name):
    parts = relative(name).parts
    require(not any(p.casefold() in {'.git', 'hooks', 'worktrees', 'modules'} for p in parts))
    require(name not in {'commondir', 'gitdir', 'objects/info/alternates', 'objects/info/http-alternates'})
    require(parts[0] in {'HEAD', 'config', 'index', 'packed-refs', 'refs', 'objects', 'logs', 'info',
        'description', 'FETCH_HEAD', 'ORIG_HEAD', 'COMMIT_EDITMSG', 'shallow'})

def restore(directory, files):
    require(isinstance(files, list) and len(files) <= 10000 and not (directory/'.git').exists())
    meta = directory/'.git'; meta.mkdir(mode=0o700); names, total = set(), 0
    for item in files:
        require(isinstance(item, dict) and set(item) <= {'path', 'kind', 'data'})
        name = item['path']; metadata_name(name); require(name.casefold() not in names); names.add(name.casefold())
        path = meta/relative(name)
        if item['kind'] == 'directory': path.mkdir(parents=True, exist_ok=True, mode=0o700)
        else:
            require(item['kind'] == 'file'); raw = base64.b64decode(item['data'], validate=True); total += len(raw); require(total <= META_LIMIT)
            path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            with open(path, 'xb') as file: file.write(raw)
            path.chmod(0o600)
    for path in [meta, *meta.rglob('*')]: require(not path.is_symlink()); os.chown(path, 1000, 1000)

def config(directory, remote):
    meta = directory/'.git'; require(meta.is_dir() and not meta.is_symlink())
    # Discard all imported aliases, includes, credential helpers, hooks, filters and URL rewrites.
    for path in meta.rglob('*'):
        name = path.relative_to(meta).as_posix(); metadata_name(name)
        info = path.lstat(); require(stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode) and info.st_nlink == 1)
        require(not path.is_symlink())
    text = '[core]\n\trepositoryformatversion = 0\n\tfilemode = false\n\tbare = false\n\tlogallrefupdates = true\n\thooksPath = /dev/null\n[remote "origin"]\n\turl = ' + remote + '\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n'
    path = meta/'config'; path.write_text(text); path.chmod(0o600); os.chown(path, 1000, 1000)

def tree_sha(directory):
    files, _ = regular_tree(directory, LIMIT, skip_git=True)
    # Exact byte tree, independent of Git filters/index/stat cache.
    digest = hashlib.sha256()
    for entry in files:
        path = entry['path'].encode(); digest.update(len(path).to_bytes(4, 'big')); digest.update(path)
        digest.update(entry['kind'].encode() + b'\0')
        if entry['kind'] == 'file': raw = base64.b64decode(entry['data'], validate=True); digest.update(len(raw).to_bytes(8, 'big')); digest.update(raw)
    return digest.hexdigest()

def api_wire(binding, action, preflight=False):
    require(action['operation'].startswith(('pr_', 'issue_')) and (not preflight or action['operation'] == 'pr_create'))
    # The daemon renders exact route/body bytes from the frozen approved action before its original admission.
    return {'channel': 'api', 'preflight': preflight}

def run(payload):
    require(os.getpid() == 1 and os.getuid() == 0 and payload['mode'] == 'github')
    nonce, session, sequence = payload['nonce'], payload['session_id'], payload['action_sequence']
    action, binding = payload['action'], payload['binding']
    go = line(); require(go == {'kind': 'go', 'nonce': nonce, 'session_id': session, 'action_sequence': sequence})
    directory = ROOT/relative(action['directory']); require(not any(p.casefold() == '.git' for p in pathlib.Path(action['directory']).parts))
    require(not any(p.name.casefold() == '.git' for p in ROOT.rglob('*')))
    bridge_lock = threading.Lock(); observed = {'count': 0, 'uncertain': False, 'mutation_sent': False, 'mutation_confirmed': False}
    def exchange(wire):
        with bridge_lock:
            observed['count'] += 1; require(observed['count'] <= 32)
            if wire.get('channel') == 'git' and wire.get('url', '').endswith('/git-receive-pack') or wire.get('channel') == 'api' and not wire.get('preflight') and action['operation'] not in {'pr_list', 'pr_view', 'issue_list', 'issue_view'}:
                observed['mutation_sent'] = True
            request = str(uuid.uuid4())
            emit({'format': 1, 'kind': 'network', 'nonce': nonce, 'session_id': session, 'action_sequence': sequence, 'request_id': request, 'network': wire})
            reply = line(); require(reply.get('kind') == 'network' and reply.get('nonce') == nonce and reply.get('session_id') == session and reply.get('action_sequence') == sequence and reply.get('request_id') == request)
            response = reply['response']; require(not response.get('uncertain') and not response.get('not_sent'))
            return response
    repo_path = '/' + binding['owner'] + '/' + binding['repository'] + '.git'
    git_root = binding['git_origin'] + repo_path
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'
        def log_message(self, *_): pass
        def do_GET(self): self.forward()
        def do_POST(self): self.forward()
        def forward(self):
            try:
                require(self.path in {repo_path + '/info/refs?service=git-upload-pack', repo_path + '/info/refs?service=git-receive-pack', repo_path + '/git-upload-pack', repo_path + '/git-receive-pack'})
                require(not self.headers.get('Transfer-Encoding') and not self.headers.get('Authorization'))
                sizes = self.headers.get_all('Content-Length', []); require(len(sizes) <= 1)
                length = int(sizes[0]) if sizes else 0; require(0 <= length <= META_LIMIT and (self.command != 'GET' or length == 0))
                body = self.rfile.read(length); require(len(body) == length)
                response = exchange({'method': self.command, 'url': binding['git_origin'] + self.path, 'body_base64': base64.b64encode(body).decode(), 'channel': 'git'})
                raw = base64.b64decode(response['body_base64'], validate=True)
                self.send_response(response['status']); self.send_header('Content-Type', response['content_type']); self.send_header('Content-Length', str(len(raw))); self.send_header('Connection', 'close'); self.end_headers(); self.wfile.write(raw)
            except Exception:
                observed['uncertain'] = True
                try: self.send_error(502, 'private_repository_broker_failed')
                except Exception: pass
    server = http.server.HTTPServer(('127.0.0.1', 0), Handler); server.timeout = .5
    thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
    remote = f'http://127.0.0.1:{server.server_port}{repo_path}'
    result = {'operation': action['operation'], 'exit_code': 1}; code = 1
    try:
        op = action['operation']
        if op in {'clone', 'open'}:
            require(not directory.exists()); directory.parent.mkdir(parents=True, exist_ok=True); os.chown(directory.parent, 1000, 1000)
            # Init + fixed config avoids Git writing sample hooks that could later be adopted.
            directory.mkdir(mode=0o700); os.chown(directory, 1000, 1000)
            succeed(directory, ['init', '--template=', '--quiet']); config(directory, remote)
            if op == 'open': succeed(directory, ['symbolic-ref', 'HEAD', 'refs/heads/main'])
            else:
                args = ['fetch', '--no-tags', '--no-recurse-submodules', 'origin']
                if action.get('branch'): args.append(action['branch'])
                succeed(directory, args)
                if action.get('branch'): succeed(directory, ['checkout', '-b', action['branch'], '--no-recurse-submodules', 'FETCH_HEAD'])
                else:
                    # Resolve the actual advertised default branch through the same scoped original broker.
                    advertised = succeed(directory, ['ls-remote', '--symref', 'origin', 'HEAD'])
                    advertised_head = next((line.split('\t')[0][5:] for line in advertised.splitlines() if line.startswith('ref: refs/heads/')), None)
                    if advertised_head:
                        require(advertised_head.startswith('refs/heads/'))
                        branch = advertised_head[len('refs/heads/'):]; succeed(directory, ['checkout', '-b', branch, '--no-recurse-submodules', 'refs/remotes/origin/' + branch])
                    else:
                        require(not advertised); succeed(directory, ['symbolic-ref', 'HEAD', 'refs/heads/main'])
        else:
            require(directory.is_dir()); restore(directory, payload['git_files']); config(directory, remote)
            succeed(directory, ['fsck', '--no-reflogs', '--connectivity-only'])
            if op in {'pull', 'checkout', 'commit', 'push'}: require(head(directory) == action['expected_head'])
            if op in {'commit', 'push'}: require(tree_sha(directory) == action['expected_tree_sha256'])
            if op == 'fetch': succeed(directory, ['fetch', '--no-tags', '--no-recurse-submodules', 'origin'])
            elif op == 'pull':
                require(not succeed(directory, ['status', '--porcelain=v1', '--untracked-files=all']))
                succeed(directory, ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', action['branch']]); succeed(directory, ['merge', '--ff-only', '--no-edit', 'FETCH_HEAD'])
            elif op == 'checkout':
                args = ['checkout', '--no-recurse-submodules']
                if action.get('create_branch', False): args.append('-b')
                args.append(action['branch']); succeed(directory, args)
            elif op == 'commit':
                succeed(directory, ['add', '--all', '--', '.'])
                succeed(directory, ['-c', 'user.name=' + action['author_name'], '-c', 'user.email=' + action['author_email'], '-c', 'commit.gpgsign=false', 'commit', '--no-verify', '--no-gpg-sign', '-m', action['message']])
            elif op == 'push':
                require(not succeed(directory, ['status', '--porcelain=v1', '--untracked-files=all']))
                # New branches explicitly approve the all-zero old OID; existing branches require nonforce ancestry.
                if action['expected_remote'] == '0' * 40:
                    require(not succeed(directory, ['ls-remote', '--heads', 'origin', 'refs/heads/' + action['branch']]))
                else:
                    succeed(directory, ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', 'refs/heads/' + action['branch']])
                    require(succeed(directory, ['rev-parse', 'FETCH_HEAD']) == action['expected_remote'])
                    succeed(directory, ['merge-base', '--is-ancestor', action['expected_remote'], action['expected_head']])
                succeed(directory, ['push', '--porcelain', '--no-verify', 'origin', action['expected_head'] + ':refs/heads/' + action['branch']])
                observed['mutation_confirmed'] = True
            elif op.startswith('pr_') or op.startswith('issue_'):
                if op == 'pr_create':
                    pre = exchange(api_wire(binding, action, True)); require(pre['status'] == 200)
                    before = parse(base64.b64decode(pre['body_base64'], validate=True)); require(before['object']['sha'] == action['expected_head'])
                response = exchange(api_wire(binding, action)); raw = base64.b64decode(response['body_base64'], validate=True)
                value = parse(raw); result.update(api_status=response['status'], response=value)
                if op == 'pr_create' and 200 <= response['status'] < 300 and value.get('head', {}).get('sha') != action['expected_head']:
                    result['uncertain'] = True
                require(200 <= response['status'] < 300)
                if not result.get('uncertain'): observed['mutation_confirmed'] = True
            else: require(op in {'status', 'close'})
        result.update(head=head(directory), branch=succeed(directory, ['symbolic-ref', '--short', 'HEAD']),
            status=succeed(directory, ['status', '--porcelain=v1', '--untracked-files=all']), tree_sha256=tree_sha(directory))
        code = 0
    except Exception:
        result['error'] = 'private_github_operation_failed'
        if observed['mutation_sent'] and not observed['mutation_confirmed']: result['uncertain'] = True
    finally:
        # Stop all tenant descendants before any metadata/workspace byte capture.
        stop_children(); server.shutdown(); server.server_close(); thread.join(timeout=5); require(not thread.is_alive())
    result['exit_code'] = code; result['uncertain'] = result.get('uncertain', False) or observed['uncertain']
    result['remote_effect_confirmed'] = observed['mutation_sent'] and observed['mutation_confirmed']
    git_files = []
    if code == 0:
        # An ephemeral bridge port is not part of durable metadata. Never store a token or external helper.
        config(directory, 'http://127.0.0.1/disabled-private-broker')
        git_files, meta_bytes = regular_tree(directory/'.git', META_LIMIT)
        files, file_bytes = regular_tree(ROOT, LIMIT, skip_git=True); require(meta_bytes + file_bytes <= LIMIT)
    else:
        files, _ = regular_tree(ROOT, LIMIT, skip_git=True)
    emit({'format': 1, 'kind': 'result', 'nonce': nonce, 'session_id': session, 'action_sequence': sequence,
          'action': action, 'result': result, 'files': files, 'git_files': git_files})
