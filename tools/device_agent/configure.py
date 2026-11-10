"""Import an exported protocol-2 profile for the standalone V2 device agent.

Creates private files only in a new, explicitly selected operator directory.
Does not install, download, start an agent, contact a server or change global state.
"""
import argparse
import datetime
import importlib.util
import json
import os
import pathlib
import re
import stat
import sys
import uuid
from urllib.parse import urlsplit


class ConfigurationError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ConfigurationError(message)


def plain_text(value, limit, label):
    require(isinstance(value, str) and value and len(value.encode()) <= limit
            and not any(ord(c) < 32 or ord(c) == 127 for c in value), 'Invalid ' + label)
    return value


def secret(value, label):
    require(isinstance(value, str) and 0 < len(value.encode()) <= 8192
            and all(33 <= ord(c) <= 126 for c in value), 'Invalid ' + label)
    return value


def ordinary(path, directory=False):
    info = path.lstat()
    require(not stat.S_ISLNK(info.st_mode) and not getattr(info, 'st_file_attributes', 0) & 0x400
            and (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)),
            'Only ordinary files and directories are allowed')
    if not directory:
        require(info.st_nlink == 1, 'Linked configuration files are not allowed')
    return info


def selected_path(value, exists=True, directory=False):
    path = pathlib.Path(value)
    require(path.is_absolute() and not str(path).startswith(('\\\\', '//')),
            'Select an absolute local operator path')
    for parent in reversed(path.parent.parents):
        ordinary(parent, True)
    ordinary(path.parent, True)
    if exists:
        ordinary(path, directory)
    return path


def read_json(path, maximum=65536):
    info = ordinary(path)
    require(info.st_size <= maximum, 'Selected JSON file is too large')
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, 'Duplicate exported profile fields')
            result[key] = value
        return result
    try:
        return json.loads(path.read_text(encoding='utf-8-sig'), object_pairs_hook=pairs,
                          parse_constant=lambda _: (_ for _ in ()).throw(ConfigurationError('Nonfinite profile value')))
    except (UnicodeError, ValueError):
        raise ConfigurationError('Invalid exported JSON profile') from None


def server_origin(value):
    value = plain_text(value, 2048, 'canonical server origin')
    parsed = urlsplit(value)
    require(parsed.hostname and (parsed.scheme == 'https'
            or parsed.scheme == 'http' and parsed.hostname in {'127.0.0.1', 'localhost', '::1'})
            and not parsed.username and not parsed.password and not parsed.query
            and not parsed.fragment and parsed.path in {'', '/'},
            'Server must be a canonical HTTPS origin (loopback HTTP is allowed)')
    try:
        parsed.port
    except ValueError:
        raise ConfigurationError('Invalid server port') from None
    return value.rstrip('/')


def uuid_value(value, label):
    try:
        return str(uuid.UUID(plain_text(value, 40, label)))
    except ValueError:
        raise ConfigurationError('Invalid ' + label) from None


def profile_configuration(profile):
    require(isinstance(profile, dict) and set(profile) <= {'format', 'protocol_version', 'schema_version',
            'server_url', 'executor_id', 'registration', 'token'}
            and {'format', 'protocol_version', 'server_url', 'registration', 'token'} <= set(profile),
            'Export a complete device profile from the device pairing screen')
    require(type(profile['format']) is int and profile['format'] == 1
            and type(profile['protocol_version']) is int and profile['protocol_version'] == 2
            and profile.get('schema_version', 2) == 2, 'Only device profile format 1 / protocol 2 is supported')
    registration = profile['registration']
    require(isinstance(registration, dict) and set(registration) <= {'schema_version', 'executor_id', 'kind',
            'pool_id', 'owner_user_id', 'display_name', 'protocol_version', 'capabilities', 'labels',
            'personal_device_remote_control', 'max_concurrent_runs'}
            and {'schema_version', 'executor_id', 'kind', 'display_name', 'protocol_version',
                 'capabilities', 'max_concurrent_runs'} <= set(registration), 'Invalid executor registration')
    require(registration['schema_version'] == 2 and registration['protocol_version'] == 2,
            'Executor registration must use schema 2 / protocol 2')
    require(type(registration['max_concurrent_runs']) is int and registration['max_concurrent_runs'] == 1,
            'V2 currently executes one concurrent run. Pair the device again with max_concurrent_runs=1; registration is never rewritten')
    identifier = uuid_value(registration['executor_id'], 'executor ID')
    require(profile.get('executor_id', identifier) == identifier, 'Executor ID does not match the exported registration')
    kind = registration['kind']
    require(kind in {'personal_device', 'managed_windows'}, 'Unsupported device agent kind')
    environment = {'COWORK_SERVER_URL': server_origin(profile['server_url']), 'COWORK_EXECUTOR_ID': identifier,
                   'COWORK_AGENT_KIND': kind, 'COWORK_EXECUTOR_NAME': plain_text(registration['display_name'], 160, 'device name')}
    if kind == 'managed_windows':
        require(os.name == 'nt', 'Configure a managed Windows agent on its dedicated Windows device')
        environment['COWORK_EXECUTOR_POOL_ID'] = uuid_value(registration.get('pool_id'), 'managed Windows pool ID')
        require(registration.get('personal_device_remote_control') is None, 'Managed Windows devices have no personal remote-control policy')
    else:
        require(registration.get('pool_id') is None, 'A personal device must not have a managed pool')
        control = registration.get('personal_device_remote_control')
        require(control in {'off', 'confirm_each_session', 'unattended'}, 'Invalid personal remote-control policy')
        environment['COWORK_PERSONAL_REMOTE_CONTROL'] = control
    capabilities = registration['capabilities']
    require(isinstance(capabilities, list) and 1 <= len(capabilities) <= 64, 'Select exact executable device capabilities')
    names = []
    for capability in capabilities:
        require(isinstance(capability, dict) and set(capability) <= {'schema_version', 'name', 'version', 'attributes'}
                and capability.get('schema_version') == 2 and isinstance(capability.get('version'), str)
                and isinstance(capability.get('attributes', {}), dict),
                'Invalid device capability descriptor')
        name = capability.get('name')
        require(isinstance(name, str) and re.fullmatch(r'[A-Za-z0-9_.-]{1,160}', name), 'Invalid device capability name')
        require(name not in names, 'Duplicate device capability')
        names.append(name)
    require(not {'desktop.windows', 'desktop.linux'} <= set(names), 'Advertise only the actual device desktop platform')
    require('desktop.windows' not in names or os.name == 'nt', 'Windows desktop capability requires Windows')
    require('desktop.linux' not in names or sys.platform.startswith('linux'), 'Linux desktop capability requires Linux')
    environment['COWORK_AGENT_CAPABILITIES'] = ','.join(names)
    if registration.get('owner_user_id') is not None:
        uuid_value(registration['owner_user_id'], 'device owner ID')
    labels = registration.get('labels', {})
    require(isinstance(labels, dict) and len(labels) <= 64
            and all(isinstance(key, str) and isinstance(value, str) for key, value in labels.items()),
            'Invalid device labels')
    return environment, secret(profile['token'], 'agent credential'), registration


def read_secret(path, label):
    path = selected_path(path)
    require(ordinary(path).st_size <= 8192, 'Selected ' + label + ' file is too large')
    try:
        return secret(path.read_text(encoding='utf-8').strip(), label)
    except UnicodeError:
        raise ConfigurationError('Invalid secret file encoding') from None


def env_value(value):
    # dotenvy 0.15 supports these four escapes inside weak (double) quotes.
    # Escape $ as well so a profile cannot interpolate inherited credentials.
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('$', '\\$') + '"'


def windows_helper():
    # Resolve only the helper shipped beside this file; also works with python -I
    # and cannot import an unrelated module from the process search path.
    spec = importlib.util.spec_from_file_location('cowork_device_private',
                                                  pathlib.Path(__file__).resolve().with_name('windows_private.py'))
    require(spec is not None and spec.loader is not None, 'Private Windows helper unavailable')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def create_directory(path):
    if os.name == 'nt':
        windows_helper().create_directory(path)
    else:
        os.mkdir(path, 0o700)
        require(stat.S_IMODE(path.stat().st_mode) == 0o700, 'Private operator directory permissions unavailable')


def create_file(path, text):
    raw = text.encode('utf-8')
    if os.name == 'nt':
        windows_helper().create_file(path, raw)
    else:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0), 0o600)
        try:
            with os.fdopen(fd, 'wb', closefd=False) as file:
                file.write(raw)
                file.flush()
                os.fsync(fd)
        finally:
            os.close(fd)


LAUNCHER = '''"""Explicitly start the standalone V2 agent with this private agent.json."""
import json, os, pathlib, subprocess, sys
root=pathlib.Path(__file__).resolve().parent
settings=json.loads((root/'configuration.json').read_text(encoding='utf-8'))
agent=settings.get('agent_binary')
if not agent or len(sys.argv)!=1:
    raise SystemExit('The selected V2 agent executable is unavailable. Import a fresh profile with --agent-binary.')
path=pathlib.Path(agent)
if not path.is_absolute() or not path.is_file() or path.is_symlink():
    raise SystemExit('The selected V2 device agent executable is unavailable.')
# The agent consumes only the explicitly selected JSON; remove inherited
# Cowork variables so a previous profile cannot select different credentials.
environment={key:value for key,value in os.environ.items() if not key.upper().startswith('COWORK_')}
raise SystemExit(subprocess.call([str(path),'--config',str(root/'agent.json')],cwd=root,env=environment))
'''


GUI_LAUNCHER = '''"""Open the local V2 setup/consent window for this private profile."""
import json, pathlib, subprocess, sys
root=pathlib.Path(__file__).resolve().parent
settings=json.loads((root/'configuration.json').read_text(encoding='utf-8'))
helper=pathlib.Path(settings['gui_helper'])
if not helper.is_absolute() or not helper.is_file() or helper.is_symlink():
    raise SystemExit('The selected local V2 GUI helper is unavailable.')
raise SystemExit(subprocess.call([sys.executable,str(helper),'--config',str(root/'agent.json')]))
'''

def model_endpoint(value):
    value = plain_text(value, 4096, 'model URL')
    parsed = urlsplit(value)
    require(parsed.hostname and (parsed.scheme == 'https' or parsed.scheme == 'http'
            and parsed.hostname in {'127.0.0.1', 'localhost', '::1'})
            and not parsed.username and not parsed.password and not parsed.query
            and not parsed.fragment, 'Model URL must use HTTPS or loopback HTTP without embedded credentials')
    try:
        parsed.port
    except ValueError:
        raise ConfigurationError('Invalid model port') from None
    return value.rstrip('/')


def discover_agent(explicit):
    if explicit:
        return selected_path(explicit)
    name = 'cowork-device-agent-v2.exe' if os.name == 'nt' else 'cowork-device-agent-v2'
    script = pathlib.Path(__file__).resolve().parent
    # Supports an extracted agent-only bundle and the full delivery bundle.
    for candidate in (script / name, script.parent / name, script.parent / 'bin' / name,
                      script / 'bin' / name, script.parent.parent / 'bin' / name):
        if candidate.is_file():
            return selected_path(str(candidate))
    return None


def selected_directory(value, fallback):
    if value:
        path = selected_path(value, directory=True)
        if os.name != 'nt':
            info = path.stat()
            require(info.st_uid == os.geteuid() and stat.S_IMODE(info.st_mode) & 0o077 == 0,
                    'Selected agent state directories must be owned by the operator and private (0700)')
        return path
    return fallback


def mcp_bindings(path):
    bindings = read_json(selected_path(path), 256 * 1024)
    require(isinstance(bindings, list) and 1 <= len(bindings) <= 8,
            'MCP bindings must be an array of 1 to 8 device-owned connections')
    names = set()
    fields = {'name', 'transport', 'endpoint', 'url', 'bearer', 'allowed_tools', 'catalog_sha256',
              'command', 'arguments', 'args', 'environment'}
    for binding in bindings:
        require(isinstance(binding, dict) and set(binding) <= fields, 'Invalid MCP connection fields')
        name = binding.get('name')
        require(isinstance(name, str) and re.fullmatch(r'[A-Za-z0-9_.-]{1,160}', name)
                and name not in names, 'MCP connection names must be unique identifiers')
        names.add(name)
        transport = binding.get('transport')
        require(transport in {'streamable_http', 'stdio'}, 'Unsupported MCP transport')
        require(not ('endpoint' in binding and 'url' in binding)
                and not ('arguments' in binding and 'args' in binding), 'Duplicate MCP alias fields')
        allowed = binding.get('allowed_tools', [])
        require(isinstance(allowed, list) and len(allowed) <= 256
                and all(isinstance(tool, str) and re.fullmatch(r'[A-Za-z0-9_.-]{1,160}', tool) for tool in allowed)
                and len(allowed) == len(set(allowed)), 'Invalid MCP allowed_tools selection')
        pin = binding.get('catalog_sha256')
        require(pin is None or isinstance(pin, str) and re.fullmatch(r'[0-9a-fA-F]{64}', pin),
                'MCP catalog_sha256 must be a SHA-256 digest')
        bearer = binding.get('bearer')
        if bearer is not None:
            secret(bearer, 'MCP bearer')
        arguments = binding.get('arguments', binding.get('args', []))
        require(isinstance(arguments, list) and len(arguments) <= 128
                and all(isinstance(argument, str) and len(argument.encode()) <= 8192 and '\0' not in argument for argument in arguments),
                'Invalid device-owned MCP command arguments')
        environment = binding.get('environment', {})
        require(isinstance(environment, dict) and len(environment) <= 64
                and all(isinstance(key, str) and re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]{0,127}', key)
                        and isinstance(value, str) and len(value.encode()) <= 8192 and '\0' not in value
                        for key, value in environment.items()), 'Invalid private MCP process environment')
        if transport == 'streamable_http':
            model_endpoint(binding.get('endpoint', binding.get('url')))
            require(binding.get('command') is None and not arguments and not environment,
                    'HTTP MCP connections do not use native command configuration')
        else:
            selected_path(plain_text(binding.get('command'), 4096, 'MCP command path'))
            require(not binding.get('endpoint', binding.get('url')) and bearer is None,
                    'stdio MCP uses a selected native command and private environment')
    return bindings


def programs(values):
    result = {}
    for value in values:
        alias, separator, path = value.partition('=')
        require(separator and re.fullmatch(r'[A-Za-z][A-Za-z0-9_-]{0,63}', alias)
                and alias not in result, 'Use unique --program alias=absolute-file selections')
        executable = selected_path(path)
        require(os.name == 'nt' or os.access(executable, os.X_OK), 'Selected native program is not executable')
        result[alias] = str(executable)
    require(len(result) <= 32, 'Select at most 32 native programs')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', required=True, help='Absolute path of the explicitly exported secret device profile')
    parser.add_argument('--output', required=True, help='Absolute NEW private operator directory; parent must exist')
    parser.add_argument('--agent-binary', help='Existing standalone V2 executable; defaults to the executable shipped beside this helper')
    parser.add_argument('--model-url', help='Explicit device-local model/LiteLLM URL')
    parser.add_argument('--model-name', help='Model name available at that endpoint')
    parser.add_argument('--model-key-file', help='Absolute private file containing the device-owned model key; optional for keyless local models')
    parser.add_argument('--github-key-file', help='Device-owned GitHub credential; copied into the new private directory')
    parser.add_argument('--workspace', help='Absolute existing dedicated workspace; defaults to a new private output/workspaces directory')
    parser.add_argument('--toolbox-data-dir', help='Absolute existing dedicated toolbox directory; defaults to new private output/toolbox')
    parser.add_argument('--program', action='append', default=[], help='Explicit native toolbox program alias=absolute-file; repeat for each program')
    parser.add_argument('--allow-native-tools', action='store_true', help='Explicitly authorize selected native tools to use the device operator\'s operating-system rights')
    parser.add_argument('--mcp-bindings-file', help='Explicit device-owned MCP bindings JSON, copied into the new private directory')
    parser.add_argument('--model-api', choices=['openai','ollama'], default='openai')
    parser.add_argument('--model-implementation', choices=['openai_compatible','ollama','vllm'], default='openai_compatible')
    parser.add_argument('--web-search-url', help='Explicit HTTPS query endpoint with exactly one {query} placeholder')
    parser.add_argument('--allowed-host', action='append', default=[], help='Operator-selected HTTPS host permitted to resolve into private address space')
    parser.add_argument('--crewai-version', help='Exact explicitly installed CrewAI distribution version')
    parser.add_argument('--crewai-source-sha256', help='Actual installed runtime inventory digest from the agent runtime probe')
    parser.add_argument('--crew-profile-binding', action='append', default=[], help='Original profile reference=operator-selected model name at this device endpoint')
    parser.add_argument('--check', action='store_true', help='Validate profile and selections; print nonsecret metadata without writing, starting or connecting')
    args = parser.parse_args()
    profile = read_json(selected_path(args.profile))
    environment, token, registration = profile_configuration(profile)
    output = selected_path(args.output, exists=False)
    require(not output.exists() and not output.is_symlink(), 'Select a new operator directory; existing data is never overwritten')
    agent = discover_agent(args.agent_binary)
    require(agent is not None or args.check, 'Standalone V2 executable not found beside this helper. Select its absolute path with --agent-binary')
    if agent is not None and os.name != 'nt':
        require(os.access(agent, os.X_OK), 'The selected V2 agent is not executable')
    model_url = model_endpoint(args.model_url) if args.model_url else None
    model_name = plain_text(args.model_name, 200, 'model name') if args.model_name else None
    require(bool(model_url)==bool(model_name), 'Select both model URL and name, or neither for a direct Office-only agent')
    require(model_url is not None or not args.model_key_file, 'A model credential needs its own explicit model endpoint')
    require(args.model_api!='ollama' or args.model_implementation=='ollama', 'Ollama native API requires the matching implementation descriptor')
    model_key = read_secret(args.model_key_file, 'model key') if args.model_key_file else None
    require(model_key is None or len(model_key.encode()) <= 4096, 'Selected model key exceeds the native model client limit')
    github_key = read_secret(args.github_key_file, 'GitHub key') if args.github_key_file else None
    workspace = selected_directory(args.workspace, output / 'workspaces')
    toolbox = selected_directory(args.toolbox_data_dir, output / 'toolbox')
    mcp = mcp_bindings(args.mcp_bindings_file) if args.mcp_bindings_file else None
    explicit_programs = programs(args.program)
    toolbox_path = toolbox / 'toolbox.json'
    existing_toolbox = None
    if args.toolbox_data_dir and toolbox_path.exists():
        existing_toolbox = read_json(toolbox_path, 256 * 1024)
        require(isinstance(existing_toolbox, dict)
                and set(existing_toolbox) <= {'programs', 'github_token_file', 'native_owner_rights','interop'}
                and isinstance(existing_toolbox.get('programs', {}), dict), 'Invalid selected toolbox configuration')
        require(not explicit_programs, 'Existing toolbox.json is never changed; choose a new toolbox directory for explicit --program selections')
        require(existing_toolbox.get('native_owner_rights', False) is args.allow_native_tools,
                'Existing toolbox rights do not match --allow-native-tools; choose a new toolbox directory')
        installed_programs = programs([alias + '=' + str(path) for alias, path in existing_toolbox.get('programs', {}).items()])
    else:
        require(not toolbox_path.exists() and not toolbox_path.is_symlink(), 'Select ordinary new toolbox configuration state')
        installed_programs = explicit_programs
    names = environment['COWORK_AGENT_CAPABILITIES'].split(',')
    require(not {'model.external','model.chat','model.ollama','model.vllm','crew.python'}.intersection(names) or model_url is not None, 'Selected model/Crew capabilities need an explicit device-owned model endpoint')
    supported = {'model.external', 'model.chat', 'tool.terminal', 'terminal', 'tool.browser', 'browser',
                 'windows.office', 'office', 'tool.github', 'github', 'tool.mcp.invoke', 'mcp', 'desktop.windows', 'desktop.linux',
                 'files','shell','git','web.fetch','office.ooxml','office.libreoffice','office.microsoft','browser.headless','browser.visible','model.ollama','model.vllm','crew.python'}
    require(set(names) <= supported, 'Pair only supported V2 model, terminal, browser, Office, GitHub, MCP or platform desktop capabilities')
    native = set(names) - {'model.external', 'model.chat','model.ollama','model.vllm'}
    require(not native or args.allow_native_tools,
            'Declared native tools require explicit --allow-native-tools authorization on this device')
    require(not {'tool.terminal', 'terminal'}.intersection(names) or installed_programs,
            'Terminal capabilities require explicitly selected native programs')
    require(not {'tool.browser', 'browser'}.intersection(names) or {'chromium','chrome','firefox','browser_host'}.intersection(installed_programs),
            'Browser capabilities require an explicit managed browser or browser_host program')
    require(not {'windows.office', 'office'}.intersection(names) or 'python' in installed_programs,
            'Office capabilities require an explicit python program; LibreOffice is additionally needed for document conversion')
    require(not {'tool.github', 'github'}.intersection(names) or 'gh' in installed_programs and github_key is not None,
            'GitHub capabilities require the gh program and device-owned --github-key-file')
    require(not {'mcp', 'tool.mcp.invoke'}.intersection(names) or mcp is not None,
            'MCP capabilities require explicitly selected device-owned MCP bindings')
    require(not mcp or args.allow_native_tools, 'Device-local MCP requires explicit --allow-native-tools authorization')
    require(not mcp or not any(binding['transport'] == 'stdio' for binding in mcp) or 'python' in installed_programs,
            'stdio MCP requires the explicitly selected python helper program')
    require('model.ollama' not in names or args.model_api=='ollama', 'model.ollama requires the native Ollama request adapter')
    require('model.vllm' not in names or args.model_api=='openai' and args.model_implementation=='vllm', 'model.vllm requires an explicitly selected vLLM-compatible deployment')
    require(not {'files','shell','git','web.fetch','office.ooxml','office.libreoffice','office.microsoft','browser.visible','crew.python'}.intersection(names) or 'python' in installed_programs, 'Interop adapters require the explicitly selected Python runtime')
    require('shell' not in names or 'shell' in installed_programs, 'shell requires an explicitly selected shell program')
    require('git' not in names or 'git' in installed_programs, 'git requires an explicitly selected Git program')
    require(not {'browser.headless','browser.visible'}.intersection(names) or 'chromium' in installed_programs, 'Select Chromium for browser interop')
    require('office.libreoffice' not in names or 'libreoffice' in installed_programs, 'Select LibreOffice for native document conversion')
    require('office.microsoft' not in names or os.name=='nt' and {'word','excel','powerpoint','powershell','python'}<=set(installed_programs), 'Microsoft interop requires Windows and all three selected Office apps, PowerShell and document inspection Python')
    if existing_toolbox is not None and not any([args.web_search_url,args.allowed_host,args.crewai_version,args.crewai_source_sha256,args.crew_profile_binding]):
        existing=existing_toolbox.get('interop',{})
        args.web_search_url=existing.get('web_search_url');args.allowed_host=existing.get('allowed_hosts',[]);args.crewai_version=existing.get('crewai_version');args.crewai_source_sha256=existing.get('crewai_source_sha256')
        args.crew_profile_binding=[ref+'='+name for ref,name in existing.get('crew_profile_bindings',{}).items()]
    if args.web_search_url:
        search=plain_text(args.web_search_url,4096,'search endpoint');parsed=urlsplit(search)
        require(parsed.scheme=='https' and parsed.hostname and not parsed.username and not parsed.password and not parsed.fragment and search.count('{query}')==1, 'Use a credential-free HTTPS search endpoint with one {query} placeholder')
    require('web.fetch' not in names or args.web_search_url is not None, 'web.fetch includes search; select the explicit search endpoint')
    require('crew.python' not in names or args.crewai_version and args.crewai_source_sha256 and re.fullmatch('[a-f0-9]{64}',args.crewai_source_sha256), 'Crew requires the exact installed runtime version and full source inventory pin')
    bindings={}
    for item in args.crew_profile_binding:
        require('=' in item,'Use profile-reference=model-name');ref,name=item.split('=',1)
        require(ref and name and ref not in bindings and len(ref)<=200 and len(name)<=200,'Use unique bounded local profile bindings');bindings[ref]=name
    require(len(bindings)<=64 and len(args.allowed_host)<=64,'Interop configuration exceeds bounds')
    for host in args.allowed_host:
        require(isinstance(host,str) and re.fullmatch('[A-Za-z0-9.-]{1,253}',host),'Select an explicit bounded hostname without a path or credentials')
    interop={'web_search_url':args.web_search_url,'allowed_hosts':args.allowed_host,'crewai_version':args.crewai_version,'crewai_source_sha256':args.crewai_source_sha256,'crew_profile_bindings':bindings}
    if existing_toolbox is not None:
        require(existing_toolbox.get('interop',{})==interop or not any([args.web_search_url,args.allowed_host,args.crewai_version,args.crewai_source_sha256,args.crew_profile_binding]), 'Existing interop policy is immutable; select a new toolbox directory')
        interop=existing_toolbox.get('interop',interop)
    configuration = {'format': 1, 'server_url': environment['COWORK_SERVER_URL'], 'registration': registration,
                     'agent_token_file': str(output / 'agent-token.txt'),
                     'model': {'base_url': model_url, 'model_name': model_name,
                               'api_key_file': str(output / 'model-key.txt') if model_key is not None else None,
                               'api':args.model_api,'implementation':args.model_implementation} if model_url is not None else None,
                     'workspace_root': str(workspace), 'toolbox_data_dir': str(toolbox),
                     'github_token_file': str(output / 'github-key.txt') if github_key is not None else None,
                     'mcp_bindings_file': str(output / 'mcp-bindings.json') if mcp is not None else None,
                     'rfb_endpoint': None}
    metadata = {'format': 1, 'protocol_version': 2, 'created_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'executor_id': environment['COWORK_EXECUTOR_ID'], 'kind': environment['COWORK_AGENT_KIND'],
                'server_url': environment['COWORK_SERVER_URL'], 'capabilities': names,
                'agent_binary': str(agent) if agent is not None else None,
                'gui_helper': str(pathlib.Path(__file__).resolve().parent / 'ui.py'),
                'model_key_configured': model_key is not None, 'github_key_configured': github_key is not None,
                'mcp_configured': mcp is not None, 'native_owner_rights': args.allow_native_tools,
                'explicit_programs': sorted(installed_programs), 'standalone_v2': True}
    if args.check:
        print(json.dumps({'checked': True, 'writes': False, 'network': False, 'started': False,
                          'agent_available': agent is not None, 'configuration': metadata}, indent=2, ensure_ascii=False))
        return
    create_directory(output)
    if not args.workspace:
        create_directory(workspace)
    if not args.toolbox_data_dir:
        create_directory(toolbox)
    if existing_toolbox is None:
        create_file(toolbox_path, json.dumps({'programs': installed_programs,
                    'github_token_file': str(output / 'github-key.txt') if github_key is not None else None,
                    'native_owner_rights': args.allow_native_tools,'interop':interop},
                    indent=2, ensure_ascii=False) + '\n')
    create_file(output / 'agent-token.txt', token + '\n')
    if model_key is not None:
        create_file(output / 'model-key.txt', model_key + '\n')
    if github_key is not None:
        create_file(output / 'github-key.txt', github_key + '\n')
    if mcp is not None:
        create_file(output / 'mcp-bindings.json', json.dumps(mcp, indent=2, ensure_ascii=False) + '\n')
    create_file(output / 'agent.json', json.dumps(configuration, indent=2, ensure_ascii=False) + '\n')
    create_file(output / '.env', '# Paths only; the standalone V2 agent is started using --config.\n'
                + 'COWORK_DEVICE_AGENT_CONFIG=' + env_value(str(output / 'agent.json')) + '\n')
    create_file(output / 'configuration.json', json.dumps(metadata, indent=2, ensure_ascii=False) + '\n')
    create_file(output / 'registration.json', json.dumps(registration, indent=2, ensure_ascii=False) + '\n')
    create_file(output / 'start.py', LAUNCHER)
    create_file(output / 'start-gui.py', GUI_LAUNCHER)
    print('Private standalone V2 device profile imported. No installation, download, start or server contact was performed.')
    print('Start explicitly using python <output>/start.py. Keep the exported profile and private output directory outside Git and shared folders.')


if __name__ == '__main__':
    try:
        main()
    except ConfigurationError as error:
        print('Device configuration refused: ' + str(error), file=sys.stderr)
        raise SystemExit(1)
    except (OSError, ValueError, UnicodeError, TypeError):
        # Never print source JSON, secret-bearing values or raw OS exceptions.
        print('Device configuration failed. Any created output directory remains private; inspect it before retrying with a new directory.', file=sys.stderr)
        raise SystemExit(1)
