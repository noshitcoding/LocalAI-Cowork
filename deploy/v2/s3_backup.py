"""Private, read-back-verified S3 content backups using the operator's AWS CLI.

Only an explicitly configured bucket/prefix is accessed. Keys never enter argv,
receipts or the inherited AWS profile chain. Restoration requires an empty target
prefix; it does not delete or overwrite another installation's objects.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import urllib.parse


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as source:
        while block := source.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def credential(path):
    path = Path(path)
    require(path.is_absolute() and path.is_file() and not path.is_symlink(), 'Select a private absolute S3 credential file')
    require(path.stat().st_size <= 16384, 'Oversized S3 credential file')
    if os.name == 'posix':
        require(path.stat().st_mode & 0o077 == 0, 'S3 credential file must be private')
    value = path.read_text(encoding='utf-8').strip()
    require(value and '\n' not in value and '\r' not in value, 'Invalid S3 credential')
    return value


class ObjectBackup:
    def __init__(self, configuration):
        self.config = configuration
        endpoint = urllib.parse.urlsplit(configuration['endpoint'])
        require(endpoint.scheme == 'https' and endpoint.hostname and endpoint.path in ('', '/')
                and not endpoint.username and not endpoint.password and not endpoint.query and not endpoint.fragment,
                'External S3 backup requires an explicit HTTPS origin')
        require(re.fullmatch(r'[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]', configuration['bucket']), 'Invalid S3 bucket')
        prefix = configuration.get('prefix', '')
        require(not prefix.startswith('/') and '\x00' not in prefix and (not prefix or prefix.endswith('/')), 'Use a complete S3 prefix ending in /')
        require(re.fullmatch(r'[a-z0-9-]{1,64}', configuration.get('region', 'us-east-1')), 'Invalid S3 region')
        self.prefix = prefix
        self.env = {name: value for name, value in os.environ.items()
                    if not name.startswith(('AWS_', 'COWORK_'))}
        self.env.update(AWS_ACCESS_KEY_ID=credential(configuration['access_key_file']),
                        AWS_SECRET_ACCESS_KEY=credential(configuration['secret_key_file']),
                        AWS_DEFAULT_REGION=configuration.get('region', 'us-east-1'),
                        AWS_EC2_METADATA_DISABLED='true', AWS_PAGER='',
                        AWS_CONFIG_FILE=os.devnull, AWS_SHARED_CREDENTIALS_FILE=os.devnull)
        if configuration.get('session_token_file'):
            self.env['AWS_SESSION_TOKEN'] = credential(configuration['session_token_file'])
        require(shutil.which('aws'), 'Install the operator AWS CLI before external S3 backup')

    def call(self, action, *arguments):
        process = subprocess.run(['aws', '--endpoint-url', self.config['endpoint'], '--no-cli-pager',
                                  's3api', action, '--bucket', self.config['bucket'], *arguments],
                                 env=self.env, stdin=subprocess.DEVNULL, capture_output=True, timeout=1800)
        # Provider diagnostics can contain credential/request material; never publish them.
        require(process.returncode == 0, 'S3 operation failed: ' + action)
        require(len(process.stdout) <= 32 * 1024 * 1024, 'S3 response exceeds backup inventory limit')
        return json.loads(process.stdout) if process.stdout.strip() else {}

    def inventory(self):
        result, token = [], None
        while True:
            arguments = ['--prefix', self.prefix, '--max-keys', '1000', '--no-paginate']
            if token:
                arguments += ['--continuation-token', token]
            page = self.call('list-objects-v2', *arguments)
            for item in page.get('Contents', []):
                require(item['Key'].startswith(self.prefix), 'S3 returned an object outside the selected prefix')
                result.append({'key': item['Key'], 'bytes': item['Size'], 'etag': item['ETag'],
                               'modified': item['LastModified']})
            require(len(result) <= 100000, 'S3 backup inventory limit exceeded')
            if not page.get('IsTruncated'):
                break
            next_token = page.get('NextContinuationToken')
            require(next_token and next_token != token, 'S3 inventory pagination did not advance')
            token = next_token
        require(len({item['key'] for item in result}) == len(result), 'Duplicate S3 inventory entry')
        return sorted(result, key=lambda item: item['key'])

    def backup(self, directory):
        directory.mkdir(mode=0o700, parents=True, exist_ok=False)
        original = self.inventory()
        entries = []
        for number, item in enumerate(original):
            target = directory / (str(number) + '.object')
            metadata = self.call('get-object', '--key', item['key'], '--if-match', item['etag'], str(target))
            require(target.is_file() and not target.is_symlink() and target.stat().st_size == item['bytes'], 'S3 object size changed')
            if os.name == 'posix':
                target.chmod(0o600)
            entries.append({**item, 'file': target.name, 'sha256': digest(target),
                            'content_type': metadata.get('ContentType', 'application/octet-stream'),
                            'metadata': metadata.get('Metadata', {})})
        require(self.inventory() == original, 'S3 contents changed during stopped backup')
        manifest = {'format': 'localai-cowork-s3-backup/1', 'bucket': self.config['bucket'],
                    'endpoint': self.config['endpoint'], 'prefix': self.prefix,
                    'scope': 'current_objects', 'objects': entries}
        path = directory / 'manifest.json'
        with path.open('x', encoding='utf-8') as output:
            json.dump(manifest, output, sort_keys=True, indent=2)
        if os.name == 'posix':
            path.chmod(0o600)
        return {'objects': len(entries), 'bytes': sum(item['bytes'] for item in entries),
                'manifest_sha256': digest(path), 'scope': 'current_objects'}

    def restore(self, directory):
        path = directory / 'manifest.json'
        require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 32 * 1024 * 1024, 'Missing S3 backup manifest')
        manifest = json.loads(path.read_bytes())
        require(manifest['format'] == 'localai-cowork-s3-backup/1' and manifest['scope'] == 'current_objects', 'Unknown S3 backup format')
        require(not self.inventory(), 'S3 restore requires an empty explicitly selected target prefix')
        entries = manifest['objects']
        require(len(entries) <= 100000, 'S3 restore inventory limit exceeded')
        require(len({item['key'] for item in entries}) == len(entries), 'Duplicate S3 restore key')
        # Validate every object before making any remote write.
        for item in entries:
            require(re.fullmatch(r'[0-9]+\.object', item['file']), 'Invalid S3 object archive filename')
            target = directory / item['file']
            require(target.is_file() and not target.is_symlink() and stat.S_ISREG(target.stat().st_mode)
                    and target.stat().st_nlink == 1 and target.stat().st_size == item['bytes']
                    and digest(target) == item['sha256'], 'S3 archive object changed')
            require(item['key'].startswith(manifest['prefix']), 'Archived S3 object escapes its prefix')
        for item in entries:
            key = self.prefix + item['key'][len(manifest['prefix']):]
            source = directory / item['file']
            self.call('put-object', '--key', key, '--body', str(source), '--if-none-match', '*',
                      '--content-type', item['content_type'], '--metadata', json.dumps(item['metadata']))
            with __import__('tempfile').TemporaryDirectory(prefix='.cowork-s3-readback-', dir=directory.parent) as temporary:
                target = Path(temporary) / 'readback'
                self.call('get-object', '--key', key, str(target))
                require(digest(target) == item['sha256'], 'Restored S3 readback mismatch; target remains retained')
        require({item['key'] for item in self.inventory()} ==
                {self.prefix + item['key'][len(manifest['prefix']):] for item in entries}, 'S3 target inventory changed during restore')
        return {'objects': len(entries), 'readback_verified': True}
