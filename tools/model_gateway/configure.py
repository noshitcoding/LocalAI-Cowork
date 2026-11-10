"""Stage an owned pinned model gateway. No model download, service start or global install.

First export the deployment manifest for independent tokenizer/template review.
Install only a reviewed bundle whose deployment digest matches that manifest.
"""
import argparse
import hashlib
import importlib.util
import ipaddress
import json
import os
from pathlib import Path
import secrets
import shutil
import stat
import struct
import subprocess


def ordinary(path, directory=False):
    path = Path(os.path.abspath(path))
    for item in (*reversed(path.parents), path):
        info = item.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
            raise ValueError('Linked path refused')
    info = path.stat()
    if not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)):
        raise ValueError('Ordinary input required')
    if not directory and info.st_nlink != 1:
        raise ValueError('Hard-linked file refused')
    return path


def sha(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(',', ':')).encode('utf-8')


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def json_input(path, limit=60 * 1024 * 1024):
    path = ordinary(path)
    if path.stat().st_size > limit:
        raise ValueError('Input exceeds reviewed bundle limit')
    def unique(pairs):
        result = {}
        for name, value in pairs:
            if name in result:
                raise ValueError('Duplicate reviewed JSON field')
            result[name] = value
        return result
    return json.loads(path.read_text(encoding='utf-8'), object_pairs_hook=unique)


def native_binary(path):
    path = ordinary(path)
    with path.open('rb') as file:
        header = file.read(64)
        if os.name == 'nt':
            if header[:2] != b'MZ' or len(header) != 64:
                raise ValueError('Native Windows x64 executable required')
            offset = struct.unpack_from('<I', header, 60)[0]
            if not 64 <= offset <= path.stat().st_size - 6:
                raise ValueError('Invalid native PE header')
            file.seek(offset)
            if file.read(6) != b'PE\0\0\x64\x86':
                raise ValueError('Native Windows x64 executable required')
        elif not (header[:6] == b'\x7fELF\x02\x01' and struct.unpack_from('<H', header, 18)[0] == 62):
            raise ValueError('Native Linux x64 executable required')
    return path


def inventory(root):
    result = {}
    for current, directories, names in os.walk(root, followlinks=False):
        for name in directories:
            ordinary(Path(current) / name, True)
        for name in names:
            path = ordinary(Path(current) / name)
            result[path.relative_to(root).as_posix()] = sha(path)
            if len(result) > 4096:
                raise ValueError('Backend runtime inventory exceeds limit')
    if not result:
        raise ValueError('Pinned native backend files required')
    return result


def manifest(args):
    root = ordinary(args.backend_directory, True)
    server = native_binary(args.server)
    gateway = native_binary(args.gateway)
    name = server.relative_to(root).as_posix()
    model = ordinary(args.model)
    if not 1024 <= args.context_tokens <= 2_000_000:
        raise ValueError('Context limit outside supported range')
    hashes = [args.tokenizer_sha256, args.chat_template_sha256]
    if any(len(h) != 64 or any(c not in '0123456789abcdef' for c in h) for h in hashes):
        raise ValueError('Exact artifact SHA-256 values required')
    if not args.model_id or len(args.model_id) > 200 or any(ord(c) < 32 for c in args.model_id):
        raise ValueError('Explicit fixed model ID required')
    return {'format': 1, 'backend': 'llama_cpp_token_ids_v1', 'supervisor': 'cowork-executor-owned-v1',
            'server': name, 'gateway_sha256': sha(gateway), 'runtime_files': inventory(root), 'model_sha256': sha(model),
            'model_id': args.model_id, 'context_tokens': args.context_tokens,
            'tokenizer_sha256': args.tokenizer_sha256, 'chat_template_sha256': args.chat_template_sha256}


def mkdir(path):
    if os.name == 'nt':
        from windows_private import create_directory
        create_directory(path)
    else:
        path.mkdir(mode=0o700)


def write(path, content):
    if os.name == 'nt':
        from windows_private import create_file
        create_file(path, content)
    else:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'wb') as file:
            file.write(content)
            file.flush()
            os.fsync(file.fileno())
        parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)


def copy_private(source, target, executable=False):
    # Destination inherits the new private directory, never a source ACL.
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if os.name != 'nt':
        flags |= os.O_NOFOLLOW
    fd = os.open(target, flags, 0o700 if executable else 0o600)
    with source.open('rb') as incoming, os.fdopen(fd, 'wb') as outgoing:
        shutil.copyfileobj(incoming, outgoing, 1024 * 1024)
        outgoing.flush()
        os.fsync(outgoing.fileno())


def install(args):
    reviewed = json_input(args.bundle)
    profile = json_input(args.profile, 64 * 1024)
    deployment = json_input(args.manifest, 1024 * 1024)
    current = argparse.Namespace(backend_directory=args.backend_directory, server=args.server, gateway=args.gateway, model=args.model,
                                 context_tokens=deployment['context_tokens'], model_id=deployment['model_id'],
                                 tokenizer_sha256=deployment['tokenizer_sha256'], chat_template_sha256=deployment['chat_template_sha256'])
    if manifest(current) != deployment or reviewed['evidence']['deployment_sha256'] != digest(deployment):
        raise ValueError('Reviewed immutable deployment differs from actual source files')
    if reviewed['provider_sha256'] != digest(profile) or reviewed['wire_mode'] != 'openai_completions':
        raise ValueError('Reviewed actual profile/raw-wire binding differs')
    if profile['kind'] != 'openai_compatible' or not profile['credential_required'] or not profile['credential_ready']:
        raise ValueError('Authenticated explicit OpenAI raw profile required')
    identity = reviewed['qualification']['model']
    if identity['provider_id'] != profile['id'] or profile['context_tokens'] != deployment['context_tokens']:
        raise ValueError('Provider identity/context differs from qualification')
    for field in ('model_id', 'model_sha256', 'context_tokens', 'tokenizer_sha256', 'chat_template_sha256'):
        if identity[field] != deployment[field]:
            raise ValueError('Reviewed model identity differs from deployment')
    host, port = args.listen.rsplit(':', 1)
    address = ipaddress.ip_address(host.strip('[]'))
    canonical_host = f'[{address.compressed}]' if address.version == 6 else address.compressed
    if not address.is_loopback or not 1 <= int(port) <= 65535 or args.listen != f'{canonical_host}:{int(port)}':
        raise ValueError('Explicit loopback listener required')
    if profile['endpoint'] != f'http://{args.listen}':
        raise ValueError('Profile must identify this exact loopback gateway')
    target = Path(args.install_directory)
    if not target.is_absolute() or target.exists():
        raise ValueError('New absolute private install directory required')
    ordinary(target.parent, True)
    source = ordinary(args.backend_directory, True)
    model = ordinary(args.model)
    gateway_source = native_binary(args.gateway)
    mkdir(target)
    backend = target / 'backend'; mkdir(backend)
    data = target / 'state'; mkdir(data)
    for name, expected in deployment['runtime_files'].items():
        destination = backend / name
        for folder in reversed(tuple(destination.parent.parents)):
            if folder == backend or backend in folder.parents:
                if not folder.exists():
                    mkdir(folder)
        if not destination.parent.exists():
            mkdir(destination.parent)
        copy_private(ordinary(source / name), destination, name == deployment['server'] or os.name != 'nt')
        if sha(destination) != expected:
            raise ValueError('Copied backend hash differs; retained staging directory needs explicit review')
    weights = target / 'model.gguf'; copy_private(model, weights)
    if sha(weights) != deployment['model_sha256']:
        raise ValueError('Copied model hash differs; retained staging directory needs explicit review')
    gateway = target / ('cowork-model-gateway.exe' if os.name == 'nt' else 'cowork-model-gateway')
    copy_private(gateway_source, gateway, True)
    if sha(gateway) != deployment['gateway_sha256'] or manifest(current) != deployment:
        raise ValueError('Original software/model changed during installation; retained staging requires review')
    token = target / 'gateway-token.txt'; write(token, secrets.token_hex(32).encode('ascii'))
    config = {'format': 1, 'listen': args.listen, 'profile': profile, 'bundle': reviewed,
              'deployment': deployment, 'backend_directory': str(backend), 'model_file': str(weights),
              'data_directory': str(data), 'bearer_file': str(token)}
    write(target / 'gateway.json', canonical(config) + b'\n')
    write(target / 'deployment.json', canonical(deployment) + b'\n')
    write(target / 'profile.json', canonical(profile) + b'\n')
    # Validate the actual retained native executable/configuration without
    # launching a backend, contacting a provider, or deriving qualification.
    subprocess.run([str(gateway), '--inspect-config', str(target / 'gateway.json')], check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
    helper = Path(__file__).with_name('sbom.py')
    if not helper.exists():
        helper = Path(__file__).parent.parent / 'release' / 'sbom.py'
    helper = ordinary(helper)
    spec = importlib.util.spec_from_file_location('cowork_installed_model_gateway_sbom', helper)
    software = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(software)
    inspected_runtime = software.runtime.capture(backend, name='owned model backend runtime')
    if {name: item['sha256'] for name, item in inspected_runtime['files'].items()} != deployment['runtime_files']:
        raise ValueError('Installed backend namespace changed during inventory')
    original_documents = []
    if args.backend_upstream_sbom:
        original_document = ordinary(args.backend_upstream_sbom)
        document = json_input(original_document)
        # Keep the original document bytes: its document checksum, namespace
        # and exact selected native artifact binding are verified by the helper.
        retained_document = target / 'BACKEND-UPSTREAM.spdx.json'
        copy_private(original_document, retained_document)
        if sha(original_document) != sha(retained_document):
            raise ValueError('Original upstream software document changed')
        original_documents.append((retained_document, deployment['runtime_files'][deployment['server']]))
    installed_sbom = software.generate(target, [backend / deployment['server']], payloads=[gateway],
                                      runtime_inventories=[inspected_runtime], include_cargo=False,
                                      include_desktop=False, include_npm=False,
                                      upstream_documents=original_documents)
    write(target / 'BACKEND-RUNTIME-INVENTORY.json', canonical(inspected_runtime) + b'\n')
    write(target / 'INSTALLED-SBOM.spdx.json', canonical(installed_sbom) + b'\n')
    print(json.dumps({'configured': True, 'config': str(target / 'gateway.json'),
                      'credential_file': str(token), 'deployment_sha256': digest(deployment),
                      'gateway': str(gateway),
                      'software_inventory': str(target / 'INSTALLED-SBOM.spdx.json'),
                      'opaque_backend_graph_attached': bool(original_documents),
                      'started': False, 'real_model_qualification_claim': False}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    for name in ('manifest', 'install'):
        command = sub.add_parser(name)
        command.add_argument('--backend-directory', required=True)
        command.add_argument('--server', required=True)
        command.add_argument('--gateway', required=True)
        command.add_argument('--model', required=True)
        if name == 'manifest':
            command.add_argument('--model-id', required=True)
            command.add_argument('--context-tokens', required=True, type=int)
            command.add_argument('--tokenizer-sha256', required=True)
            command.add_argument('--chat-template-sha256', required=True)
            command.add_argument('--output', required=True)
        else:
            command.add_argument('--bundle', required=True)
            command.add_argument('--manifest', required=True)
            command.add_argument('--profile', required=True)
            command.add_argument('--listen', required=True)
            command.add_argument('--install-directory', required=True)
            command.add_argument('--backend-upstream-sbom', help='Optional original SPDX 2.3 document bound to the exact selected native server SHA256')
    args = parser.parse_args()
    if args.command == 'manifest':
        result = manifest(args)
        output = Path(args.output)
        if not output.is_absolute():
            raise ValueError('Absolute new manifest output required')
        ordinary(output.parent, True)
        write(output, canonical(result) + b'\n')
        print(json.dumps({'deployment_sha256': digest(result), 'real_model_qualification_claim': False}))
    else:
        install(args)


if __name__ == '__main__':
    main()
