"""Typed, opt-in transport for one dedicated local QEMU executor.

The operator owns these endpoints. They never come from an operation payload.
Only a public CA is copied into the readonly guest state share. Backup, restore,
update and enrollment retain their separate qualification gates.
"""
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import ssl
import uuid
from urllib.parse import parse_qsl, urlsplit

import executor_host


class GuestEndpointError(Exception):
    pass


def require(condition, code):
    if not condition:
        raise GuestEndpointError(code)


def port(value):
    require(type(value) is int and 1024 <= value <= 65535, 'invalid_guest_endpoint_port')
    return value


def runtime_url(value):
    require(isinstance(value, str), 'invalid_guest_runtime_endpoint')
    try:
        parsed = urlsplit(value)
        require(parsed.scheme == 'https' and parsed.hostname == '10.0.2.2'
                and port(parsed.port) and parsed.path in ('', '/')
                and not parsed.username and not parsed.password and not parsed.query
                and not parsed.fragment, 'invalid_guest_runtime_endpoint')
    except (ValueError, TypeError):
        raise GuestEndpointError('invalid_guest_runtime_endpoint') from None
    return value.rstrip('/')


def public_ca(path):
    path = Path(path)
    require(path.is_absolute() and path.is_file() and not path.is_symlink()
            and all(not item.is_symlink() for item in path.parents), 'unsafe_guest_public_ca')
    value = path.stat()
    require(value.st_nlink == 1 and 0 < value.st_size <= 262144, 'unsafe_guest_public_ca')
    if os.name == 'posix':
        require(value.st_uid == os.geteuid() and not value.st_mode & 0o022, 'unsafe_guest_public_ca')
    content = path.read_bytes()
    require(b'-----BEGIN CERTIFICATE-----' in content and b'PRIVATE KEY' not in content,
            'guest_public_ca_contains_private_material')
    require(re.fullmatch(rb'(?:\s*-----BEGIN CERTIFICATE-----\s+[A-Za-z0-9+/=\s]+-----END CERTIFICATE-----\s*)+', content),
            'guest_public_ca_contains_noncertificate_material')
    try:
        ssl.create_default_context(cadata=content.decode('ascii'))
    except (ValueError, UnicodeError, ssl.SSLError):
        raise GuestEndpointError('invalid_guest_public_ca') from None
    return content


def settings(config):
    value = config.get('guest_runtime_endpoints')
    if value is None:
        return None
    keys = {'version', 'instance_id', 'executor_host_binding', 'public_ca_file', 'database_endpoint',
            's3_endpoint', 'fleet_management_url', 'api_loopback_port'}
    require(isinstance(value, dict) and set(value) == keys and type(value['version']) is int
            and value['version'] == 1, 'invalid_guest_endpoint_profile')
    require(isinstance(value['public_ca_file'], str), 'invalid_guest_endpoint_profile')
    binding = executor_host.manager_binding(config)
    require(binding is not None and value['executor_host_binding'] == binding,
            'guest_endpoint_host_binding_mismatch')
    try:
        identity = uuid.UUID(value['instance_id'])
        require(str(identity) == value['instance_id'] and identity.int != 0,
                'invalid_guest_instance_binding')
    except (ValueError, TypeError, AttributeError):
        raise GuestEndpointError('invalid_guest_instance_binding') from None
    require(config.get('sandbox_security_profile') == 'linux_apparmor',
            'guest_requires_enforced_apparmor')
    require(isinstance(value['database_endpoint'], str)
            and re.fullmatch(r'10\.0\.2\.2:[0-9]+', value['database_endpoint']),
            'invalid_guest_database_endpoint')
    port(int(value['database_endpoint'].split(':')[1]))
    port(value['api_loopback_port'])
    for name in ['database_endpoint', 's3_endpoint', 'fleet_management_url']:
        require(config.get(name) == value[name], 'guest_endpoint_configuration_mismatch')
    runtime_url(value['s3_endpoint'])
    runtime_url(value['fleet_management_url'])
    public_ca(value['public_ca_file'])
    return dict(value)


def bind(infrastructure):
    marker = infrastructure.root / 'guest-runtime-endpoints.json'
    selected = settings(infrastructure.config)
    require(not marker.is_symlink(), 'unsafe_guest_endpoint_binding')
    if selected is None:
        require(not marker.exists(), 'guest_endpoint_profile_disable_requires_review')
        return
    evidence = {key: value for key, value in selected.items() if key != 'public_ca_file'}
    evidence['public_ca_sha256'] = hashlib.sha256(public_ca(selected['public_ca_file'])).hexdigest()
    if marker.exists():
        try:
            require(marker.is_file() and marker.stat().st_nlink == 1 and marker.stat().st_size <= 65536
                    and json.loads(marker.read_text()) == evidence, 'guest_endpoint_retarget_requires_review')
        except (ValueError, OSError):
            raise GuestEndpointError('invalid_guest_endpoint_binding') from None
    else:
        require(not any(path.is_dir() and re.fullmatch('[0-9a-f]{32}', path.name)
                        for path in infrastructure.root.iterdir()), 'guest_endpoint_existing_inventory_requires_review')
        infrastructure.private_write(marker, json.dumps(evidence, sort_keys=True).encode())


def require_instance(config, instance):
    selected = settings(config)
    if selected is not None:
        require(instance['id'] == selected['instance_id'], 'guest_single_instance_profile_scope_mismatch')


def database_url(config, value):
    if settings(config) is None:
        return value
    require('?' not in value and '#' not in value, 'invalid_guest_database_url')
    return value + '?sslmode=verify-full&sslrootcert=/run/secrets/control_public_ca'


def controller_pg_environment(config, url, environment):
    """Use the operator TLS endpoint with the runtime role, never admin elevation.

    10.0.2.2 and /run/secrets only exist inside the guest. Native dump/restore
    tools must connect to the independently configured private admin transport.
    The runtime role/password/database are retained; only the transport changes.
    """
    import instance_disk_quotas
    qualified=instance_disk_quotas.controller_pg_environment(config,url,environment)
    if qualified is not None:return qualified
    selected = settings(config)
    if selected is None:
        return environment
    try:
        source = urlsplit(url)
        admin_path = Path(config['database_admin_url_file'])
        require(admin_path.is_absolute() and admin_path.is_file() and not admin_path.is_symlink()
                and all(not parent.is_symlink() for parent in admin_path.parents), 'unsafe_guest_admin_database_file')
        info = admin_path.stat()
        require(info.st_nlink == 1 and 0 < info.st_size <= 4096, 'unsafe_guest_admin_database_file')
        if os.name == 'posix':
            require(info.st_uid == os.geteuid() and not info.st_mode & 0o077, 'unsafe_guest_admin_database_file')
        admin_url = admin_path.read_text().strip()
        admin = urlsplit(admin_url)
        require(admin.scheme in ['postgres', 'postgresql'] and admin.hostname in ['localhost', '127.0.0.1']
                and port(admin.port) and admin.username and admin.password and admin.path
                and not admin.query and not admin.fragment, 'invalid_guest_controller_database_endpoint')
        if url != admin_url:
            require(source.scheme in ['postgres', 'postgresql'] and source.hostname == '10.0.2.2'
                    and str(source.port) == selected['database_endpoint'].split(':')[1]
                    and dict(parse_qsl(source.query, strict_parsing=True)) == {
                        'sslmode': 'verify-full', 'sslrootcert': '/run/secrets/control_public_ca'}
                    and len(parse_qsl(source.query)) == 2 and not source.fragment
                    and re.fullmatch('ci_[a-f0-9]{32}', environment['PGUSER'])
                    and environment['PGDATABASE'] == environment['PGUSER'],
                    'guest_controller_database_scope_mismatch')
        result = {key: value for key, value in environment.items() if not key.startswith('PG')}
        result.update({key: environment[key] for key in ['PGUSER', 'PGPASSWORD', 'PGDATABASE']})
        result.update({'PGHOST': admin.hostname, 'PGPORT': str(admin.port), 'PGSSLMODE': 'verify-full',
                       'PGSSLROOTCERT': selected['public_ca_file'], 'PGGSSENCMODE': 'disable',
                       'PGCONNECT_TIMEOUT': '10'})
        return result
    except (ValueError, OSError, KeyError):
        raise GuestEndpointError('invalid_guest_controller_database_endpoint') from None


def controller_storage_environment(config):
    """Verify the private operator mc alias and trust only the public fixture CA."""
    selected = settings(config)
    if selected is None:
        return None
    try:
        root = Path(config['s3_admin_config_directory'])
        file = root / 'config.json'
        state = Path(config['state_directory']).resolve()
        require(root.is_absolute() and root.is_dir() and not root.is_symlink()
                and all(not parent.is_symlink() for parent in root.parents)
                and not root.resolve().is_relative_to(state) and not state.is_relative_to(root.resolve())
                and file.is_file() and not file.is_symlink(), 'unsafe_guest_storage_admin_configuration')
        info = file.stat()
        require(info.st_nlink == 1 and 0 < info.st_size <= 262144, 'unsafe_guest_storage_admin_configuration')
        if os.name == 'posix':
            require(info.st_uid == os.geteuid() and root.stat().st_uid == os.geteuid()
                    and not info.st_mode & 0o077 and not root.stat().st_mode & 0o077,
                    'unsafe_guest_storage_admin_configuration')
        alias = config['s3_admin_alias']
        require(isinstance(alias, str) and re.fullmatch('[a-zA-Z][a-zA-Z0-9_-]*', alias), 'invalid_guest_storage_admin_alias')
        values = json.loads(file.read_text())
        value = values['aliases'][alias]
        endpoint = urlsplit(value['url'])
        require(endpoint.scheme == 'https' and endpoint.hostname in ['localhost', '127.0.0.1']
                and port(endpoint.port) and endpoint.path in ['', '/'] and not endpoint.query and not endpoint.fragment
                and not endpoint.username and not endpoint.password
                and isinstance(value['accessKey'], str) and 0 < len(value['accessKey']) <= 4096
                and isinstance(value['secretKey'], str) and 0 < len(value['secretKey']) <= 4096,
                'invalid_guest_storage_admin_endpoint')
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith('MC_') and key not in ['SSL_CERT_FILE', 'SSL_CERT_DIR']}
        environment['SSL_CERT_FILE'] = selected['public_ca_file']
        return environment
    except (KeyError, ValueError, OSError, TypeError):
        raise GuestEndpointError('invalid_guest_storage_admin_configuration') from None


def copy_public_ca(config, directory, private_write):
    selected = settings(config)
    if selected is None:
        return
    destination = directory / 'runtime-secrets' / 'control_public_ca'
    require(not destination.is_symlink(), 'unsafe_guest_public_ca_destination')
    private_write(destination, public_ca(selected['public_ca_file']))
    destination.chmod(0o444)


def apply_manifest(config, manifest, directory):
    selected = settings(config)
    if selected is None:
        return manifest
    result = copy.deepcopy(manifest)
    require(set(result['services']) == {'api', 'worker', 'runner', 'egress-proxy'},
            'guest_service_inventory_mismatch')
    require_instance(config, {'id': result['services']['api']['environment']['COWORK_INSTANCE_ID']})
    for name in ['api', 'worker']:
        service = result['services'][name]
        require('ports' not in service, 'guest_api_port_already_configured')
        service['secrets'] = [*service['secrets'], 'control_public_ca']
        service['environment'].update({
            'COWORK_FLEET_CA_CERTIFICATE_FILE': '/run/secrets/control_public_ca',
            'COWORK_S3_CA_CERTIFICATE_FILE': '/run/secrets/control_public_ca',
        })
    result['secrets']['control_public_ca'] = {'file': str(directory / 'runtime-secrets/control_public_ca')}
    result['services']['api']['ports'] = [{'target': 8080, 'published': str(selected['api_loopback_port']),
                                        'host_ip': '127.0.0.1', 'protocol': 'tcp'}]
    return result
