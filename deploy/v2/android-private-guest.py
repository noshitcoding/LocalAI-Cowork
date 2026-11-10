"""One private actor's ephemeral gVisor workspace. No host mounts or daemon credentials."""
import base64, hashlib, json, os, pathlib, selectors, shutil, signal, stat, subprocess, sys, tempfile, time

ROOT = pathlib.Path('/workspace')
LIMIT = 16 * 1024 * 1024

def require(value):
    if not value: raise ValueError('invalid_private_guest_request')

def pairs(values):
    result = {}
    for key, value in values:
        require(key not in result); result[key] = value
    return result

def parse(raw):
    return json.loads(raw, object_pairs_hook=pairs)

def line(limit):
    # The controller waits for ready before sending any MCP bytes.
    result = bytearray()
    while True:
        value = os.read(0, 8192)
        require(value)
        position = value.find(b'\n')
        if position >= 0:
            require(position == len(value) - 1 and len(result) + position <= limit)
            result.extend(value[:position]); return bytes(result)
        require(len(result) + len(value) <= limit); result.extend(value)

def relative(path):
    require(isinstance(path, str) and path and len(path.encode()) <= 2000)
    require('\\' not in path and ':' not in path and not any(ord(c) < 32 for c in path))
    parts = path.split('/'); require(all(p and p not in {'.', '..'} for p in parts))
    return ROOT.joinpath(*parts)

def import_files(values):
    require(isinstance(values, list) and len(values) <= 10000)
    names, total = set(), 0
    for value in values:
        require(isinstance(value, dict) and set(value) <= {'path', 'kind', 'data'})
        path = relative(value.get('path')); key = value['path'].casefold()
        require(key not in names); names.add(key)
        if value.get('kind') == 'directory': path.mkdir(parents=True, exist_ok=True, mode=0o700)
        else:
            require(value.get('kind') == 'file' and isinstance(value.get('data'), str))
            raw = base64.b64decode(value['data'], validate=True); total += len(raw); require(total <= LIMIT)
            path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            with open(path, 'xb') as file: file.write(raw)
            os.chmod(path, 0o600)
    for path in [ROOT, *ROOT.rglob('*')]:
        require(not path.is_symlink()); os.chown(path, 1000, 1000)

def terminate_children(primary=None):
    # Every other process belongs to this exact PID namespace. PID 1 supervises all guest descendants.
    require(os.getpid() == 1)
    try: os.kill(-1, signal.SIGKILL)
    except ProcessLookupError: pass
    if primary is not None: primary.wait(timeout=5)
    deadline = time.monotonic() + 5
    while True:
        try:
            while True:
                pid, _ = os.waitpid(-1, os.WNOHANG)
                if not pid: break
        except ChildProcessError: pass
        live = []
        for item in pathlib.Path('/proc').iterdir():
            if item.name.isdigit() and int(item.name) != 1:
                try:
                    state = (item / 'stat').read_text().rsplit(')', 1)[1].split()[0]
                    if state != 'Z': live.append(item.name)
                except FileNotFoundError: pass
        if not live: return
        require(time.monotonic() < deadline); time.sleep(.01)

def export_files():
    values, names, total = [], set(), 0
    for path in sorted(ROOT.rglob('*')):
        info = path.lstat(); relative_name = path.relative_to(ROOT).as_posix(); relative(relative_name)
        require(not path.is_symlink() and relative_name.casefold() not in names); names.add(relative_name.casefold())
        require(len(names) <= 10000)
        if stat.S_ISDIR(info.st_mode): values.append({'path': relative_name, 'kind': 'directory'})
        else:
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1)
            with open(path, 'rb') as file: raw = file.read(LIMIT + 1)
            require(len(raw) == info.st_size); total += len(raw); require(total <= LIMIT)
            values.append({'path': relative_name, 'kind': 'file', 'data': base64.b64encode(raw).decode()})
    return values

def program(payload):
    command, arguments = payload['program'], payload['arguments']; timeout = payload['timeout_seconds']
    office_action = payload.get('office_action'); office_stdout = bytearray()
    if office_action is not None:
        require(isinstance(office_action, dict) and command == '/opt/cowork/python/bin/python' and arguments == [])
        with tempfile.NamedTemporaryFile(dir='/tmp', prefix='cowork-private-office-', suffix='.json', delete=False) as action_file:
            action_file.write(json.dumps(office_action, ensure_ascii=False, separators=(',', ':')).encode()); action_path = action_file.name
        os.chown(action_path, 1000, 1000); os.chmod(action_path, 0o600)
        arguments = ['-I', '/opt/cowork-private-tools/office.py', str(ROOT), '@' + action_path]
    require(isinstance(command, str) and command.startswith('/') and '\0' not in command)
    require(isinstance(arguments, list) and len(arguments) <= 1024 and all(isinstance(v, str) and '\0' not in v for v in arguments))
    require(sum(len(v.encode()) for v in arguments) <= 32000 and isinstance(timeout, int) and 1 <= timeout <= 1800)
    child = subprocess.Popen([command, *arguments], cwd=ROOT, env={'PATH': '/opt/cowork/python/bin:/usr/local/bin:/usr/bin:/bin', 'LANG': 'C.UTF-8', 'HOME': '/tmp', 'TMPDIR': '/tmp'}, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True, user=1000, group=1000, extra_groups=[], close_fds=True)
    selector = selectors.DefaultSelector(); captures = {'stdout': bytearray(), 'stderr': bytearray()}; totals = {'stdout': 0, 'stderr': 0}
    for read, name in ((child.stdout, 'stdout'), (child.stderr, 'stderr')):
        os.set_blocking(read.fileno(), False); selector.register(read, selectors.EVENT_READ, name)
    start = time.monotonic(); timed_out = False
    while selector.get_map():
        if time.monotonic() - start >= timeout:
            timed_out = True; child.kill(); terminate_children(child)
        for key, _ in selector.select(timeout=.05):
            raw = os.read(key.fileobj.fileno(), 8192)
            if not raw: selector.unregister(key.fileobj); key.fileobj.close(); continue
            totals[key.data] += len(raw); available = max(0, 65536 - len(captures[key.data])); captures[key.data].extend(raw[:available])
            if office_action is not None and key.data == 'stdout': office_stdout.extend(raw[:max(0, 8 * 1024 * 1024 - len(office_stdout))])
        if child.poll() is not None: terminate_children(child)
    code = child.wait(); terminate_children(child)
    result = {'exit_code': code, 'process_id': child.pid, 'timed_out': timed_out, 'duration_ms': int((time.monotonic() - start) * 1000), 'output_complete': True}
    for name, capture in captures.items(): result[name] = {'base64': base64.b64encode(capture).decode(), 'sha256': hashlib.sha256(capture).hexdigest(), 'total_bytes': totals[name], 'truncated': totals[name] > len(capture)}
    if office_action is not None:
        pathlib.Path(action_path).unlink(missing_ok=True)
        return result, base64.b64encode(office_stdout).decode()
    return result, None

def processes():
    values = []
    for item in pathlib.Path('/proc').iterdir():
        if item.name.isdigit() and int(item.name) != 1:
            try: values.append((int(item.name), (item / 'stat').read_text().rsplit(')', 1)[1].split()[0]))
            except FileNotFoundError: pass
    return values

def pause_children():
    deadline = time.monotonic() + 5
    while True:
        values = processes()
        for pid, state in values:
            if state not in {'T', 't', 'Z'}:
                try: os.kill(pid, signal.SIGSTOP)
                except ProcessLookupError: pass
        if all(state in {'T', 't', 'Z'} for _, state in processes()): return
        require(time.monotonic() < deadline); time.sleep(.005)

def resume_children():
    for pid, state in processes():
        if state in {'T', 't'}:
            try: os.kill(pid, signal.SIGCONT)
            except ProcessLookupError: pass

def emit(value):
    print(json.dumps(value, ensure_ascii=False, separators=(',', ':')), flush=True)

def mcp(payload):
    command, arguments, environment = payload['command'], payload['arguments'], payload['environment']
    require(isinstance(command, str) and command.startswith('/') and '\0' not in command)
    require(isinstance(arguments, list) and len(arguments) <= 128 and all(isinstance(v, str) and '\0' not in v for v in arguments))
    require(isinstance(environment, dict) and len(environment) <= 64)
    for key, value in environment.items(): require(isinstance(key, str) and key and key.replace('_', 'A').isalnum() and isinstance(value, str) and '\0' not in value)
    env = {'PATH': '/opt/cowork/python/bin:/usr/local/bin:/usr/bin:/bin', 'LANG': 'C.UTF-8', 'HOME': '/tmp', 'TMPDIR': '/tmp', **environment}
    child = subprocess.Popen([command, *arguments], cwd=ROOT, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, user=1000, group=1000, extra_groups=[], close_fds=True)
    selector = selectors.DefaultSelector(); buffers = {'request': bytearray(), 'response': bytearray()}; seen = set()
    for stream, name in ((sys.stdin.buffer, 'request'), (child.stdout, 'response')):
        os.set_blocking(stream.fileno(), False); selector.register(stream, selectors.EVENT_READ, name)
    try:
        while selector.get_map():
            for key, _ in selector.select(timeout=.2):
                raw = os.read(key.fileobj.fileno(), 8192)
                if not raw:
                    if key.data == 'request': return
                    require(not buffers['response']); selector.unregister(key.fileobj); continue
                buffer = buffers[key.data]; buffer.extend(raw)
                require(len(buffer) <= (24 * 1024 * 1024 if key.data == 'request' else 4 * 1024 * 1024))
                while b'\n' in buffer:
                    position = buffer.index(b'\n'); value = parse(bytes(buffer[:position])); del buffer[:position + 1]
                    require(isinstance(value, dict))
                    if key.data == 'response': emit({'kind': 'rpc', 'value': value}); continue
                    require(value.get('nonce') == payload['nonce'])
                    if value.get('kind') == 'rpc':
                        require(isinstance(value.get('value'), dict) and child.poll() is None)
                        resume_children(); child.stdin.write(json.dumps(value['value'], ensure_ascii=False, separators=(',', ':')).encode() + b'\n'); child.stdin.flush()
                    else:
                        require(value.get('kind') == 'workspace' and value.get('operation') in {'prepare', 'capture'})
                        control_id = value['id']; require(isinstance(control_id, str) and len(control_id) == 36 and control_id not in seen and len(seen) < 1024); seen.add(control_id)
                        pause_children()
                        if value['operation'] == 'prepare':
                            for path in ROOT.iterdir():
                                if path.is_symlink() or not path.is_dir(): path.unlink()
                                else: shutil.rmtree(path)
                            import_files(value['files'])
                        files = export_files()
                        emit({'kind': 'workspace', 'id': control_id, 'nonce': payload['nonce'], 'operation': value['operation'], 'paused': True, 'server_alive': child.poll() is None, 'files': files})
    finally: terminate_children(child)

def browser(payload):
    child=subprocess.Popen(['/opt/cowork/python/bin/python','-I','/opt/cowork-private-tools/browser.py'],cwd=ROOT,env={'PATH':'/opt/cowork/python/bin:/usr/bin:/bin','LANG':'C.UTF-8','HOME':'/tmp','TMPDIR':'/tmp'},stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,user=1000,group=1000,extra_groups=[],close_fds=True)
    selector=selectors.DefaultSelector();buffers={'controller':bytearray(),'browser':bytearray()};seen=set();pending=set();active=None
    for stream,name in ((sys.stdin.buffer,'controller'),(child.stdout,'browser')):
        os.set_blocking(stream.fileno(),False);selector.register(stream,selectors.EVENT_READ,name)
    def forward(value):child.stdin.write(json.dumps(value,ensure_ascii=False,separators=(',',':')).encode()+b'\n');child.stdin.flush()
    try:
        while True:
            for key,_ in selector.select(timeout=.2):
                raw=os.read(key.fileobj.fileno(),8192);require(raw);buffer=buffers[key.data];buffer.extend(raw);require(len(buffer)<=6*1024*1024)
                while b'\n' in buffer:
                    pos=buffer.index(b'\n');value=parse(bytes(buffer[:pos]));del buffer[:pos+1];require(isinstance(value,dict))
                    if key.data=='controller':
                        require(value.get('nonce')==payload['nonce'])
                        if value.get('kind')=='action':
                            require(active is None and isinstance(value.get('sequence'),int) and value['sequence'] not in seen and len(seen)<256)
                            require(value.get('session_id')==payload['session_id'] and isinstance(value.get('action'),dict))
                            seen.add(value['sequence']);active=value['sequence'];pending.clear();resume_children();forward({'kind':'action','action':value['action']})
                        else:
                            require(value.get('kind')=='network' and active is not None and value.get('sequence')==active and value.get('request_id') in pending)
                            pending.remove(value['request_id']);forward(value)
                    else:
                        require(active is not None)
                        if value.get('kind')=='network':
                            request=value.get('request_id');require(isinstance(request,str) and len(request)==36 and request not in pending and len(pending)<512)
                            pending.add(request);emit({'kind':'network','nonce':payload['nonce'],'session_id':payload['session_id'],'action_sequence':active,'request_id':request,'network':value['network']})
                        else:
                            require(value.get('kind')=='result' and not pending)
                            pause_children();files=export_files();emit({'format':1,'kind':'result','nonce':payload['nonce'],'session_id':payload['session_id'],'action_sequence':active,'paused':True,'result':value['result'],'files':files});active=None
    finally:terminate_children(child)

def main():
    require(os.getpid() == 1 and os.getuid() == 0 and ROOT.is_dir() and not ROOT.is_symlink())
    payload = parse(line(24 * 1024 * 1024)); require(isinstance(payload, dict) and payload.get('format') == 1)
    import_files(payload.get('files', [])); nonce = payload['nonce']; require(isinstance(nonce, str) and len(nonce) == 36)
    print(json.dumps({'format': 1, 'ready': True, 'nonce': nonce}, separators=(',', ':')), flush=True)
    if payload['mode'] == 'mcp': mcp(payload)
    elif payload['mode'] == 'browser': browser(payload)
    elif payload['mode'] == 'terminal':
        import runpy
        runpy.run_path('/opt/cowork-private-tools/terminal.py')['run'](payload, globals())
    elif payload['mode'] == 'github':
        import runpy
        runpy.run_path('/opt/cowork-private-tools/github.py')['run'](payload)
    elif payload['mode'] == 'crew':
        import runpy
        runpy.run_path('/opt/cowork-private-tools/crew.py')['run'](payload, globals())
    elif payload['mode'] == 'program':
        result, office_stdout = program(payload); files = export_files()
        emit({'format': 1, 'nonce': nonce, 'result': result, 'files': files, 'office_stdout': office_stdout})
    else: raise ValueError('unknown_private_guest_mode')

if __name__ == '__main__':
    try: main()
    except Exception:
        try: terminate_children()
        finally: sys.exit(70)
