"""Pinned CrewAI child. The original supervisor is the only model/tool transport."""
import base64, hashlib, importlib.metadata, json, os, pathlib, selectors, subprocess, sys, threading, uuid
MAX_FRAME = 4 * 1024 * 1024

def encode(value):
    raw = json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
    if len(raw) > MAX_FRAME: raise ValueError('private_crew_frame_bound')
    return raw

def inventory():
    dist = importlib.metadata.distribution('crewai')
    root = pathlib.Path(dist.locate_file('')).resolve(strict=True)
    files, total = [], 0
    for item in sorted(dist.files or [], key=str):
        rel = pathlib.PurePosixPath(str(item).replace('\\', '/'))
        if rel.is_absolute() or '..' in rel.parts or not rel.parts: continue
        if not (rel.parts[0] == 'crewai' or rel.parts[0].lower().startswith('crewai-') and rel.parts[0].endswith('.dist-info')): continue
        if rel.suffix in ('.pyc', '.pyo'): continue
        p = pathlib.Path(dist.locate_file(item))
        if p.is_symlink() or not p.is_file() or not p.resolve().is_relative_to(root): raise ValueError('private_crew_distribution_scope')
        raw = p.read_bytes(); total += len(raw)
        if total > 512 * 1024 * 1024 or len(files) >= 10000: raise ValueError('private_crew_distribution_bound')
        files.append([str(item), len(raw), hashlib.sha256(raw).hexdigest()])
    if not files: raise ValueError('private_crew_distribution_empty')
    return {'version': dist.version, 'source_sha256': hashlib.sha256(encode(files)).hexdigest()}

def child():
    raw = sys.stdin.buffer.readline(MAX_FRAME + 1)
    if len(raw) > MAX_FRAME or not raw.endswith(b'\n'): raise ValueError('private_crew_bootstrap_bound')
    config = json.loads(raw)
    expected = config['runtime']
    if inventory() != {'version': expected['crewai_version'], 'source_sha256': expected['crewai_source_sha256']}: raise ValueError('private_crew_original_runtime_changed')
    os.environ.update({'OTEL_SDK_DISABLED': 'true', 'CREWAI_TELEMETRY_DISABLED': 'true', 'DO_NOT_TRACK': '1'})
    from crewai import Agent, BaseLLM, Crew, Process, Task
    from crewai.tools import BaseTool
    from pydantic import BaseModel, Field, PrivateAttr
    send_lock = threading.Lock()
    def rpc(kind, body):
        source = str(uuid.uuid4())
        request = {'format': 1, 'source': source, 'kind': kind, 'node_id': config['node_id'], 'role_id': config['role_id'], 'original_plan_sha256': config['plan_sha256'], 'body': body}
        with send_lock:
            sys.stdout.buffer.write(encode(request) + b'\n'); sys.stdout.buffer.flush()
            raw = sys.stdin.buffer.readline(MAX_FRAME + 1)
            if len(raw) > MAX_FRAME or not raw.endswith(b'\n'): raise ValueError('private_crew_original_response_bound')
            response = json.loads(raw)
            if response.get('source') != source or response.get('request_sha256') != hashlib.sha256(encode(request)).hexdigest(): raise ValueError('private_crew_original_response_binding')
            if not response.get('ok'): raise RuntimeError('private_crew_original_effect_unresolved_no_replay')
            return response['result']
    class ActorLLM(BaseLLM):
        def __init__(self): super().__init__(model='actor-private-bound-model', temperature=0)
        def supports_function_calling(self): return False
        def supports_stop_words(self): return False
        def get_context_window_size(self): return config['context_tokens']
        def call(self, messages, tools=None, callbacks=None, available_functions=None, **kwargs):
            if isinstance(messages, str): messages = [{'role': 'user', 'content': messages}]
            result = rpc('model', {'messages': messages})
            content = result.get('content')
            if not isinstance(content, str): raise ValueError('private_crew_original_model_content_missing')
            return content
    class Arguments(BaseModel):
        arguments: dict = Field(default_factory=dict, description='Exact current Actor-private tool arguments.')
    class ActorTool(BaseTool):
        args_schema: type[BaseModel] = Arguments
        _tool: str = PrivateAttr()
        def __init__(self, name):
            super().__init__(name=name, description='Call the Actor-private '+name+' adapter. Concrete mutations need their exact current human approval. Original current schema: '+json.dumps(next((t for t in config['tool_catalog'] if t.get('function',{}).get('name')==name),{}),ensure_ascii=False), args_schema=Arguments)
            self._tool = name
        def _run(self, arguments=None, **kwargs): return json.dumps(rpc('tool', {'name': self._tool, 'arguments': arguments or {}}), ensure_ascii=False)
    role = config['role']
    # Dependency, concurrency, hierarchy and delegation admission are owned by
    # the durable host graph. This child performs exactly its original claimed node.
    agent = Agent(role=role['role'], goal=role['goal'], backstory=role.get('backstory', ''), llm=ActorLLM(), tools=[ActorTool(n) for n in config['allowed_tools']], allow_delegation=False, max_iter=config['max_steps'], max_retry_limit=int(config.get('retry_count') or 0), max_rpm=config.get('max_rpm'), max_execution_time=config['remaining_seconds'], cache=False, verbose=False)
    task = Task(description=config['goal'], expected_output=config.get('expected_output', 'Return the complete result for this original node.'), agent=agent, guardrail_max_retries=0)
    result = Crew(agents=[agent], tasks=[task], process=Process.sequential, memory=False, cache=False, verbose=False, planning=False).kickoff()
    output = result.raw
    if len(output.encode('utf-8')) > 1024 * 1024: raise ValueError('private_crew_node_output_bound')
    rpc('result', {'content': output})
    sys.stdout.buffer.write(encode({'format': 1, 'kind': 'finished', 'node_id': config['node_id'], 'content_sha256': hashlib.sha256(output.encode('utf-8')).hexdigest(), 'runtime': inventory()}) + b'\n')
    sys.stdout.buffer.flush()

def run(payload, supervisor):
    expected = payload['runtime']
    if hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest() != expected['bridge_sha256']: raise ValueError('private_crew_original_bridge_changed')
    if inventory() != {'version': expected['crewai_version'], 'source_sha256': expected['crewai_source_sha256']}: raise ValueError('private_crew_original_runtime_changed')
    child_process = subprocess.Popen(['/opt/cowork/python/bin/python', '-I', __file__, '--child'], cwd='/workspace', env={'PATH': '/opt/cowork/python/bin:/usr/bin:/bin', 'LANG': 'C.UTF-8', 'HOME': '/tmp', 'TMPDIR': '/tmp', 'PYTHONUNBUFFERED': '1', 'OTEL_SDK_DISABLED': 'true', 'CREWAI_TELEMETRY_DISABLED': 'true', 'DO_NOT_TRACK': '1'}, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, user=1000, group=1000, extra_groups=[], close_fds=True)
    child_process.stdin.write(encode(payload['node']) + b'\n'); child_process.stdin.flush()
    sel = selectors.DefaultSelector(); buffers = {'controller': bytearray(), 'child': bytearray()}; pending = None; seen = set()
    for stream, name in ((sys.stdin.buffer, 'controller'), (child_process.stdout, 'child')):
        os.set_blocking(stream.fileno(), False); sel.register(stream, selectors.EVENT_READ, name)
    try:
        while True:
            for key, _ in sel.select(timeout=.2):
                raw = os.read(key.fileobj.fileno(), 8192)
                if not raw: raise ValueError('private_crew_original_pipe_closed')
                buffer = buffers[key.data]; buffer.extend(raw)
                if len(buffer) > MAX_FRAME: raise ValueError('private_crew_original_frame_bound')
                while b'\n' in buffer:
                    pos = buffer.index(b'\n'); value = json.loads(bytes(buffer[:pos])); del buffer[:pos+1]
                    if key.data == 'child':
                        if value.get('kind') == 'finished':
                            if pending is not None or value.get('node_id') != payload['node']['node_id']: raise ValueError('private_crew_original_finish_binding')
                            supervisor['pause_children']()
                            supervisor['emit']({'format': 1, 'kind': 'crew_finished', 'nonce': payload['nonce'], 'node_id': value['node_id'], 'content_sha256': value['content_sha256'], 'runtime': value['runtime']})
                            return
                        source = value.get('source')
                        if pending is not None or not isinstance(source, str) or str(uuid.UUID(source)) != source or source in seen or len(seen) >= 10000: raise ValueError('private_crew_original_source_binding')
                        if value.get('node_id') != payload['node']['node_id'] or value.get('role_id') != payload['node']['role_id'] or value.get('original_plan_sha256') != payload['node']['plan_sha256'] or value.get('kind') not in ('model', 'tool', 'result'): raise ValueError('private_crew_original_node_binding')
                        seen.add(source); pending = value
                        supervisor['pause_children']()
                        original = encode(value)
                        supervisor['emit']({'format': 1, 'kind': 'crew_request', 'nonce': payload['nonce'], 'request': value, 'original_request_base64': base64.b64encode(original).decode('ascii'), 'request_sha256': hashlib.sha256(original).hexdigest()})
                    else:
                        if pending is None or value.get('nonce') != payload['nonce'] or value.get('kind') != 'crew_response' or value.get('source') != pending['source'] or value.get('request_sha256') != hashlib.sha256(encode(pending)).hexdigest(): raise ValueError('private_crew_original_reply_binding')
                        response = {'source': value['source'], 'request_sha256': value['request_sha256'], 'ok': value['ok'], 'result': value.get('result')}
                        supervisor['resume_children'](); child_process.stdin.write(encode(response) + b'\n'); child_process.stdin.flush(); pending = None
    finally: supervisor['terminate_children'](child_process)

if __name__ == '__main__':
    if len(sys.argv) == 2 and sys.argv[1] == '--inventory': print(json.dumps(inventory(), separators=(',', ':')))
    elif len(sys.argv) == 2 and sys.argv[1] == '--child': child()
    else: raise ValueError('private_crew_explicit_mode_required')
