#!/usr/bin/env python3
"""Prepare an immutable domain/TLS instance gateway; never deploy or request certificates."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import ssl
import stat
import subprocess
from urllib.parse import urlsplit


class InvalidConfiguration(Exception):
    pass


def require(condition, message):
    if not condition:
        raise InvalidConfiguration(message)


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, 'Duplicate configuration key')
        value[key] = item
    return value


def domain(value):
    require(isinstance(value, str) and 3 <= len(value) <= 219 and '.' in value
            and all(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', label)
                    for label in value.split('.')), 'An exact lowercase DNS domain is required')
    require(value != 'localhost' and not value.endswith('.localhost'), 'Production domains cannot be localhost')
    return value


def path(value):
    require(isinstance(value, str) and not any(c in value for c in "\r\n\0'$\""), 'Invalid operator path')
    selected = Path(value)
    require(selected.is_absolute() and '..' not in selected.parts, 'Operator paths must be absolute')
    require(all(not part.is_symlink() for part in (selected, *selected.parents)), 'Operator paths cannot traverse symlinks')
    if os.name == 'nt':
        require(all(not part.exists() or not (part.stat().st_file_attributes & 0x400)
                    for part in (selected, *selected.parents)), 'Operator paths cannot traverse reparse points')
    return selected


def read_file(selected, limit, private=False):
    selected = path(str(selected))
    descriptor = os.open(selected, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(descriptor, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 < info.st_size <= limit,
                'Operator input must be a bounded regular file')
        if private and os.name == 'posix':
            require(info.st_uid == os.geteuid() and not info.st_mode & 0o077,
                    'Private key must be owned by the operator and have no group/other permissions')
        data = stream.read(limit + 1)
        require(len(data) == info.st_size, 'Operator input changed while reading')
        return data


def write_file(selected, content):
    descriptor = os.open(selected, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())


def command(arguments):
    try:
        result = subprocess.run(arguments, capture_output=True, timeout=20, check=False)
    except (OSError, subprocess.SubprocessError):
        raise InvalidConfiguration('Required local certificate/ACL validator is unavailable') from None
    require(result.returncode == 0, 'Local certificate/ACL validation failed')
    return result.stdout


def configuration(raw):
    keys = {'format', 'management_origin', 'instance_domain', 'certificate_file', 'private_key_file',
            'management_secrets', 'platform_policy_file', 'server_image', 'web_image',
            'services_network', 'routing_network', 'provisioner_configuration_file', 'instance_passkeys_enabled'}
    require(isinstance(raw, dict) and set(raw) == keys and type(raw['format']) is int and raw['format'] == 1,
            'Unsupported gateway configuration')
    require(isinstance(raw['management_origin'], str), 'Management origin must be a string')
    parsed = urlsplit(raw['management_origin'])
    require(parsed.scheme == 'https' and not parsed.username and not parsed.password
            and parsed.port is None and parsed.path == '' and not parsed.query and not parsed.fragment,
            'Management origin must be an exact HTTPS DNS origin without a port or path')
    host = domain(parsed.hostname)
    require(raw['management_origin'] == 'https://' + host, 'Management origin must be canonical')
    instance_domain = domain(raw['instance_domain'])
    require(type(raw['instance_passkeys_enabled']) is bool, 'Instance passkeys setting must be explicit')
    require(not re.fullmatch(r'[0-9a-f]{32}\.' + re.escape(instance_domain), host), 'Management and instance hosts overlap')
    for name in ('server_image', 'web_image'):
        require(isinstance(raw[name], str) and re.fullmatch(r'[a-z0-9][a-z0-9.:/_-]*@sha256:[0-9a-f]{64}', raw[name]),
                'Current server and web images must use immutable repository digests')
    for name in ('services_network', 'routing_network'):
        require(isinstance(raw[name], str) and re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_.-]{0,62}', raw[name]), 'Invalid network name')
    require(raw['services_network'] != raw['routing_network'], 'Service and public routing networks must be separate')
    for name in ('certificate_file', 'private_key_file', 'management_secrets', 'platform_policy_file', 'provisioner_configuration_file'):
        path(raw[name])
    return host, instance_domain


def render(template, management, instances):
    require(template.count('__MANAGEMENT_HOST__') == 2 and template.count('__INSTANCE_DOMAIN_REGEX__') == 2,
            'Unexpected gateway template contract')
    result = template.replace('__MANAGEMENT_HOST__', management).replace('__INSTANCE_DOMAIN_REGEX__', re.escape(instances))
    require('__' not in result, 'An unrendered gateway placeholder remains')
    return result.encode('utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path, help='New private directory; existing output is never overwritten')
    args = parser.parse_args()
    raw = json.loads(read_file(args.config, 65536), object_pairs_hook=unique_object)
    management, instances = configuration(raw)
    certificate = read_file(raw['certificate_file'], 262144)
    require(b'PRIVATE KEY' not in certificate, 'Certificate chain must not contain private material')
    private_key = read_file(raw['private_key_file'], 65536, private=True)
    policy = read_file(raw['platform_policy_file'], 65536)
    require(isinstance(json.loads(policy, object_pairs_hook=unique_object), dict), 'Platform policy must be a reviewed JSON object')
    provisioner_bytes = read_file(raw['provisioner_configuration_file'], 1024 * 1024, private=True)
    provisioner = json.loads(provisioner_bytes, object_pairs_hook=unique_object)
    require(isinstance(provisioner, dict) and isinstance(provisioner.get('images'), dict)
            and provisioner['images'].get('server') == raw['server_image']
            and provisioner.get('services_network') == raw['services_network']
            and provisioner.get('routing_network') == raw['routing_network'],
            'Provisioner image and network configuration must match the selected gateway release')
    provisioner['instance_gateway'] = {'version': 1, 'domain': instances, 'passkeys_enabled': raw['instance_passkeys_enabled']}
    secrets = path(raw['management_secrets'])
    require(secrets.is_dir(), 'Management secrets directory is absent')
    for name in ('management_database_url', 'bootstrap_token', 'audit_key', 'auth_master_key', 'provisioner_token'):
        read_file(secrets / name, 65536, private=True)
    output = path(str(args.output))
    require(output != Path(output.anchor) and not output.exists(), 'Output must be a new non-root directory')
    output.mkdir(mode=0o700, parents=False)
    if os.name == 'nt':
        account = command(['whoami']).decode().strip()
        command(['icacls', str(output), '/inheritance:r', '/grant:r', account + ':(OI)(CI)F', 'SYSTEM:(OI)(CI)F'])
    write_file(output / 'fullchain.pem', certificate)
    write_file(output / 'privkey.pem', private_key)
    # Validate the exact copied bytes. No listener, network connection or certificate issuance.
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(str(output / 'fullchain.pem'), str(output / 'privkey.pem'))
    cert_path = str(output / 'fullchain.pem')
    command(['openssl', 'x509', '-in', cert_path, '-noout', '-checkend', '86400'])
    alternate_names = command(['openssl', 'x509', '-in', cert_path, '-noout', '-ext', 'subjectAltName']).decode()
    dns_names = [name.lower() for name in re.findall(r'DNS:([a-zA-Z0-9*.-]+)', alternate_names)]
    require('*.' + instances in dns_names, 'Certificate must contain the exact wildcard instance DNS SAN')
    for host in (management, '0' * 32 + '.' + instances, 'f' * 32 + '.' + instances):
        result = command(['openssl', 'x509', '-in', cert_path, '-noout', '-checkhost', host]).decode()
        require('does match certificate' in result and 'does NOT match' not in result,
                'Certificate must cover management and wildcard instance origins')
    not_before = command(['openssl', 'x509', '-in', cert_path, '-noout', '-startdate']).decode().strip()
    require(datetime.strptime(not_before.removeprefix('notBefore='), '%b %d %H:%M:%S %Y %Z').replace(tzinfo=timezone.utc) <= datetime.now(timezone.utc),
            'Certificate is not yet valid')
    repo = Path(__file__).resolve().parent.parent
    template_file = repo / 'deploy/enterprise/production-nginx.conf.template'
    compose_file = repo / 'deploy/enterprise/docker-compose.production.yml'
    nginx = render(template_file.read_text(encoding='utf-8'), management, instances)
    write_file(output / 'nginx.conf', nginx)
    write_file(output / 'docker-compose.yml', compose_file.read_bytes())
    write_file(output / 'platform-policy.json', policy)
    write_file(output / 'executor_configuration.json', (json.dumps(provisioner, sort_keys=True, indent=2) + '\n').encode())
    env = {
        'COWORK_SERVER_IMAGE': raw['server_image'], 'COWORK_WEB_IMAGE': raw['web_image'],
        'COWORK_PUBLIC_ORIGIN': raw['management_origin'], 'COWORK_INSTANCE_GATEWAY_DOMAIN': instances,
        'COWORK_INSTANCE_URL_TEMPLATE': 'https://{instance}.' + instances,
        'COWORK_SERVICES_NETWORK': raw['services_network'], 'COWORK_ROUTING_NETWORK': raw['routing_network'],
        'COWORK_MANAGEMENT_SECRETS': secrets.as_posix(), 'COWORK_GATEWAY_NGINX': (output / 'nginx.conf').as_posix(),
        'COWORK_GATEWAY_CERTIFICATE': (output / 'fullchain.pem').as_posix(),
        'COWORK_GATEWAY_PRIVATE_KEY': (output / 'privkey.pem').as_posix(),
        'COWORK_PLATFORM_POLICY_FILE': (output / 'platform-policy.json').as_posix(),
    }
    require(all(not any(c in value for c in "\r\n\0'") for value in env.values()), 'Invalid environment value')
    write_file(output / 'gateway.env', ''.join(f"{key}='{value}'\n" for key, value in sorted(env.items())).encode())
    sha = lambda data: hashlib.sha256(data).hexdigest()
    receipt = {
        'format': 1, 'management_origin': raw['management_origin'], 'instance_origin_template': env['COWORK_INSTANCE_URL_TEMPLATE'],
        'api_path': '/api/v1', 'backend': 'compose_fixed_instance_routing_alias',
        'certificate_sha256': sha(certificate), 'certificate_key_pair_validated': True,
        'files': {name: sha((output / name).read_bytes()) for name in ('nginx.conf', 'docker-compose.yml', 'gateway.env', 'platform-policy.json', 'executor_configuration.json')},
        'provisioner_original_configuration_sha256': sha(provisioner_bytes),
        'instance_gateway_profile': provisioner['instance_gateway'],
        'source_pins': {'scripts/prepare-enterprise-gateway.py': sha(Path(__file__).read_bytes()),
                        'deploy/enterprise/production-nginx.conf.template': sha(template_file.read_bytes()),
                        'deploy/enterprise/docker-compose.production.yml': sha(compose_file.read_bytes()),
                        **{name: sha((repo / name).read_bytes()) for name in (
                            'server/cowork-server/src/instance_gateway.rs', 'server/cowork-server/src/main.rs',
                            'server/cowork-server/src/management.rs', 'server/cowork-server/src/fleet.rs',
                            'server/cowork-server/src/fleet_content.rs', 'server/cowork-server/src/auth.rs',
                            'server/cowork-provisioner/instance_gateway.py', 'server/cowork-provisioner/provisioner.py',
                            'server/cowork-provisioner/instance_updates.py', 'server/cowork-provisioner/Dockerfile')}},
        'deployed': False, 'runtime_qualified': False, 'production_qualified': False,
    }
    write_file(output / 'PREPARATION-RECEIPT.json', (json.dumps(receipt, sort_keys=True, indent=2) + '\n').encode())
    print('Prepared private gateway configuration: ' + str(output))


if __name__ == '__main__':
    try:
        main()
    except (InvalidConfiguration, OSError, ValueError, ssl.SSLError):
        raise SystemExit('Gateway preparation failed: invalid or unavailable operator configuration; no deployment was performed') from None
