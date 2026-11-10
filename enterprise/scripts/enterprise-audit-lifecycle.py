#!/usr/bin/env python3
"""Independent archive witness and recoverable management retention operator.

The witness must run under a separate operator on a different host/storage trust
domain. Its private RSA key and archive are never mounted into management.
Expose its loopback HTTP listener only through an operator HTTPS/mTLS gateway.
"""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import hmac
import json
import os
from pathlib import Path
import re
import ssl
import subprocess
import uuid
from urllib.request import Request, build_opener, HTTPSHandler, HTTPRedirectHandler
from urllib.parse import urlsplit

def require(ok, message):
    if not ok:
        raise RuntimeError(message)

def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(',', ':')).encode()

def sha(value):
    return hashlib.sha256(value).hexdigest()

def private(path, directory=False):
    path = Path(path)
    require(path.is_absolute() and not path.is_symlink() and all(not p.is_symlink() for p in path.parents), 'unsafe private path')
    require(path.is_dir() if directory else path.is_file(), 'missing private file/directory')
    if os.name == 'posix':
        require(path.stat().st_uid == os.getuid() and path.stat().st_mode & 0o077 == 0, 'private path requires owner-only permissions')
    return path

def write_new(path, data):
    with os.fdopen(os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), 'wb') as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())

def replace(path, data):
    temp = path.with_name('.' + path.name + '.' + uuid.uuid4().hex)
    write_new(temp, data)
    os.replace(temp, path)
    if os.name == 'posix':
        fd = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

@contextmanager
def exclusive(root):
    require(os.name == 'posix', 'operator locking requires Linux')
    import fcntl
    fd = os.open(root / '.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd,'a+b') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield

def anchor(root, key, witness_id, export):
    doc = export['document']
    export_sha = export['export_sha256']
    require(sha(canonical(doc)) == export_sha and doc['format'] == 'cowork-management-audit-export-v1', 'invalid original export')
    ident, org = str(uuid.UUID(doc['id'])), str(uuid.UUID(doc['organisation_id']))
    require(ident == doc['id'] and org == doc['organisation_id'], 'noncanonical export scope')
    with exclusive(root):
        target = root / (ident + '.json')
        # One durable object contains archive and original signature, so a lost
        # response or interrupted head update resumes the same authority.
        if target.exists():
            original = json.loads(private(target).read_bytes())
            require(original['export'] == export, 'original witness export differs')
            receipt = original['receipt']
        else:
            head_path = root / (org + '.head.json')
            head = json.loads(private(head_path).read_bytes()) if head_path.exists() else None
            previous, sequence = doc['base']['digest'], doc['base']['sequence']
            seen = {sequence: previous}
            for record in doc['records']:
                require(record['previous_digest'] == previous and record['sequence'] > sequence and record['payload']['organisation_id'] == org, 'archive chain discontinuity')
                previous, sequence = record['digest'], record['sequence']
                seen[sequence] = previous
            require(previous == doc['end_digest'] and sequence == doc['end_sequence'], 'archive head mismatch')
            require(doc['prune_through'] in seen, 'retention checkpoint absent from export')
            if head:
                require(doc['base']['sequence'] == head['checkpoint_sequence'] and doc['base']['digest'] == head['checkpoint_digest'], 'external witness rejects checkpoint rollback/fork')
                require(seen.get(head['end_sequence']) == head['end_digest'], 'external witness rejects prior head rollback/fork')
            payload = {'format': 'cowork-audit-witness-v1', 'witness_id': witness_id, 'export_id': ident,
                       'organisation_id': org, 'export_sha256': export_sha, 'base_digest': doc['base']['digest'],
                       'end_digest': previous, 'end_sequence': sequence, 'checkpoint_sequence': doc['prune_through'],
                       'checkpoint_digest': seen[doc['prune_through']], 'anchored_at': datetime.now(timezone.utc).isoformat()}
            result = subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(key)], input=canonical(payload), capture_output=True, timeout=15)
            require(result.returncode == 0, 'external witness signing failed')
            receipt = {'payload': payload, 'signature_hex': result.stdout.hex()}
            write_new(target, canonical({'export': export, 'receipt': receipt}))
        # Recover only the corresponding original head; replay of an older receipt
        # cannot move a later external checkpoint backwards.
        head_path = root / (org + '.head.json')
        current = json.loads(private(head_path).read_bytes()) if head_path.exists() else None
        p = receipt['payload']
        if current is None or current['export_id'] == ident or (current['checkpoint_digest'] == doc['base']['digest'] and current['end_sequence'] <= p['end_sequence']):
            replace(head_path, canonical(p))
        return receipt

def serve(args):
    root, key = private(args.archive, True), private(args.signing_key)
    token = private(args.token_file).read_text().strip()
    require(len(token) >= 32 and re.fullmatch('[A-Za-z0-9_-]{1,64}', args.witness_id), 'invalid witness identity/credential')
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_POST(self):
            try:
                require(self.path == '/anchors' and hmac.compare_digest(self.headers.get('Authorization', ''), 'Bearer ' + token), 'denied')
                length = int(self.headers.get('Content-Length', '0'))
                require(0 < length <= 32*1024*1024, 'invalid export size')
                receipt = anchor(root, key, args.witness_id, json.loads(self.rfile.read(length)))
                body, status = canonical(receipt), 200
            except Exception:
                body, status = b'{"error":"witness request rejected"}', 409
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
    HTTPServer(('127.0.0.1', args.port), Handler).serve_forever()

def http(origin, path, token, body=None, ca=None):
    parsed = urlsplit(origin)
    require(parsed.scheme == 'https' and parsed.hostname and not parsed.username and not parsed.query and not parsed.fragment and parsed.path in ('','/'), 'operator endpoints must be HTTPS origins')
    request = Request(origin.rstrip('/') + path, data=None if body is None else canonical(body), headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
    class NoRedirect(HTTPRedirectHandler):
        def redirect_request(self,*_):
            raise RuntimeError('operator credentials must not follow redirects')
    opener = build_opener(NoRedirect(),HTTPSHandler(context=ssl.create_default_context(cafile=ca)))
    with opener.open(request,timeout=60) as response:
        data = response.read(32*1024*1024+1)
        require(len(data) <= 32*1024*1024, 'response too large')
        return json.loads(data)

def operate(args):
    root = private(args.state, True)
    token = private(args.token_file).read_text().strip()
    prefix = '/management/v1/organisations/' + str(uuid.UUID(args.organisation)) + '/audit'
    with exclusive(root):
        plan_path = root / 'original-export.json'
        intent_path = root / 'original-intent.json'
        if not intent_path.exists():
            checkpoint = http(args.management, prefix + '/checkpoint', token, ca=args.management_ca)
            write_new(intent_path, canonical({'request_id': str(uuid.uuid4()), 'expected_revision': checkpoint['revision'], 'max_records': args.max_records,
                                             'management': args.management, 'organisation': args.organisation, 'witness': args.witness}))
        intent = json.loads(private(intent_path).read_bytes())
        require(all(intent[k] == getattr(args,k) for k in ('management','organisation','witness')), 'original operator scope differs')
        if not plan_path.exists():
            export = http(args.management, prefix + '/exports', token, {k:intent[k] for k in ('request_id','expected_revision','max_records')}, args.management_ca)
            write_new(plan_path, canonical(export))
        export = json.loads(private(plan_path).read_bytes())
        if args.action == 'plan':
            print(canonical({'export_id':export['document']['id'],'export_sha256':export['export_sha256'],'prune_through':export['document']['prune_through'],'records':len(export['document']['records'])}).decode())
            return
        witness_path = root / 'original-witness.json'
        if not witness_path.exists():
            receipt = http(args.witness, '/anchors', private(args.witness_token_file).read_text().strip(), export, args.witness_ca)
            write_new(witness_path, canonical(receipt))
        witness = json.loads(private(witness_path).read_bytes())
        receipt_path = root / 'FINAL-RECEIPT.json'
        result = http(args.management, prefix + '/exports/' + export['document']['id'] + '/complete', token,
                      {'export_sha256':export['export_sha256'],'witness_receipt':witness,'confirmation':'ANCHOR_AND_APPLY_AUDIT_RETENTION'}, args.management_ca)
        if receipt_path.exists():
            require(json.loads(private(receipt_path).read_bytes()) == result, 'original final receipt differs')
        else:
            write_new(receipt_path, canonical(result))
        print(canonical(result).decode())

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest='command', required=True)
    witness = subs.add_parser('witness')
    witness.add_argument('--archive', required=True)
    witness.add_argument('--signing-key', required=True)
    witness.add_argument('--token-file', required=True)
    witness.add_argument('--witness-id', required=True)
    witness.add_argument('--port', type=int, default=18095)
    operator = subs.add_parser('operator')
    operator.add_argument('action', choices=['plan','apply'])
    for name in ('management','organisation','witness','state','token-file'):
        operator.add_argument('--'+name, required=True)
    operator.add_argument('--witness-token-file')
    operator.add_argument('--management-ca')
    operator.add_argument('--witness-ca')
    operator.add_argument('--max-records', type=int, default=1000)
    args = parser.parse_args()
    if args.command == 'operator' and args.action == 'apply':
        require(args.witness_token_file is not None, 'apply requires witness credential')
    serve(args) if args.command == 'witness' else operate(args)

if __name__ == '__main__':
    main()
