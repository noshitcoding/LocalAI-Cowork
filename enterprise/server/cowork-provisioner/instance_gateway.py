"""Typed operator-owned browser origins. Operation payloads never select an origin or RP."""
import re
import uuid
from urllib.parse import urlsplit


class GatewayError(Exception):
    pass


def require(condition, code):
    if not condition:
        raise GatewayError(code)


def settings(config):
    profile = config.get('instance_gateway')
    if profile is None:
        return None
    require(isinstance(profile, dict) and set(profile) == {'version', 'domain', 'passkeys_enabled'}
            and type(profile['version']) is int and profile['version'] == 1
            and type(profile['passkeys_enabled']) is bool, 'invalid_instance_gateway_profile')
    domain = profile['domain']
    require(isinstance(domain, str) and len(domain) <= 219 and '.' in domain
            and not domain.endswith('.localhost')
            and all(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', label)
                    for label in domain.split('.')), 'invalid_instance_gateway_domain')
    return dict(profile)


def desktop_origins(config, origin):
    configured = config.get('desktop_origins', '')
    require(isinstance(configured, str), 'invalid_instance_gateway_desktop_origins')
    origins = [origin]
    for value in configured.split(','):
        value = value.strip()
        if not value:
            continue
        try:
            parsed = urlsplit(value)
            port = parsed.port
        except ValueError:
            raise GatewayError('invalid_instance_gateway_desktop_origins') from None
        host = '[' + parsed.hostname + ']' if parsed.hostname and ':' in parsed.hostname else parsed.hostname
        canonical = parsed.scheme + '://' + (host or '') + (':' + str(port) if port is not None else '')
        require(parsed.hostname and not parsed.username and not parsed.password and parsed.path == ''
                and not parsed.query and not parsed.fragment
                and (parsed.scheme == 'https' or parsed.scheme == 'http' and parsed.hostname in ('localhost', '127.0.0.1', '::1'))
                and value.isascii() and value == canonical
                and port not in (80 if parsed.scheme == 'http' else 443,),
                'invalid_instance_gateway_desktop_origins')
        if value not in origins:
            origins.append(value)
    return ','.join(origins)


def environment(config, instance):
    profile = settings(config)
    if profile is None:
        return {}
    try:
        identity = uuid.UUID(instance['id'])
        require(identity.int != 0, 'invalid_instance_gateway_identity')
    except (KeyError, ValueError, TypeError, AttributeError):
        raise GatewayError('invalid_instance_gateway_identity') from None
    host = identity.hex + '.' + profile['domain']
    origin = 'https://' + host
    values = {'COWORK_PUBLIC_ORIGIN': origin,
              'COWORK_INSTANCE_GATEWAY_DOMAIN': profile['domain'],
              'COWORK_INSTANCE_URL_TEMPLATE': 'https://{instance}.' + profile['domain'],
              'COWORK_DESKTOP_ORIGINS': desktop_origins(config, origin)}
    if profile['passkeys_enabled']:
        # An RP cannot inherit the management domain or another instance's credentials.
        values['COWORK_WEBAUTHN_RP_ID'] = host
    return values


def validate_manifest(config, instance, manifest):
    expected = environment(config, instance)
    if not expected:
        return
    try:
        actual = manifest['services']['api']['environment']
        require(all(actual.get(key) == value for key, value in expected.items()),
                'instance_gateway_runtime_origin_mismatch')
        if 'COWORK_WEBAUTHN_RP_ID' not in expected:
            require('COWORK_WEBAUTHN_RP_ID' not in actual, 'instance_gateway_runtime_rp_mismatch')
    except (KeyError, TypeError):
        raise GatewayError('instance_gateway_runtime_origin_mismatch') from None
