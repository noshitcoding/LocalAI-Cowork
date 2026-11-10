"""Narrow offline crypto/continuity checks; no Docker, guest or server actions."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
import uuid

def module(name):
    spec = importlib.util.spec_from_file_location(name,Path(__file__).with_name(name+'.py'))
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded

audit = module('enterprise-audit-lifecycle')
cert = module('enterprise-certificate-lifecycle')

@unittest.skipUnless(os.name=='posix' and shutil.which('openssl'),'Linux OpenSSL checks')
class OperatorCryptoChecks(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='cowork-operator-crypto-')
        self.root = Path(self.temporary.name)
        self.root.chmod(0o700)
        self.key = self.root/'key.pem'
        subprocess.run(['openssl','genpkey','-algorithm','RSA','-pkeyopt','rsa_keygen_bits:3072','-out',str(self.key)],check=True,capture_output=True)
        self.key.chmod(0o600)
        self.archive = self.root/'archive'; self.archive.mkdir(mode=0o700)
        self.org = str(uuid.uuid4())
    def tearDown(self):
        self.temporary.cleanup()
    def export(self,base_sequence=0,base_digest='0'*64,end_sequence=1,end_digest='a'*64):
        doc={'format':'cowork-management-audit-export-v1','id':str(uuid.uuid4()),'organisation_id':self.org,
             'base':{'sequence':base_sequence,'digest':base_digest},'records':[{'sequence':end_sequence,'digest':end_digest,
             'previous_digest':base_digest,'payload':{'organisation_id':self.org}}],'end_sequence':end_sequence,'end_digest':end_digest,'prune_through':end_sequence}
        return {'document':doc,'export_sha256':audit.sha(audit.canonical(doc))}
    def test_original_witness_retry_is_identical_and_real_signature_validates(self):
        original=self.export()
        receipt=audit.anchor(self.archive,self.key,'independent-1',original)
        self.assertEqual(receipt,audit.anchor(self.archive,self.key,'independent-1',original))
        public=self.root/'public.pem'
        subprocess.run(['openssl','pkey','-in',str(self.key),'-pubout','-out',str(public)],check=True,capture_output=True)
        signature=self.root/'signature.bin'; signature.write_bytes(bytes.fromhex(receipt['signature_hex']))
        check=subprocess.run(['openssl','dgst','-sha256','-verify',str(public),'-signature',str(signature)],input=audit.canonical(receipt['payload']),capture_output=True)
        self.assertEqual(check.returncode,0)
        mutated={**receipt['payload'],'export_sha256':'f'*64}
        check=subprocess.run(['openssl','dgst','-sha256','-verify',str(public),'-signature',str(signature)],input=audit.canonical(mutated),capture_output=True)
        self.assertNotEqual(check.returncode,0)
    def test_witness_rejects_chain_fork_and_checkpoint_rollback(self):
        first=self.export(); audit.anchor(self.archive,self.key,'independent-1',first)
        with self.assertRaises(RuntimeError):
            audit.anchor(self.archive,self.key,'independent-1',self.export())
        next_export=self.export(1,'a'*64,2,'b'*64)
        audit.anchor(self.archive,self.key,'independent-1',next_export)
        audit.anchor(self.archive,self.key,'independent-1',first)
        head=json.loads((self.archive/(self.org+'.head.json')).read_bytes())
        self.assertEqual(head['end_sequence'],2)
    def test_witness_rejects_original_export_tampering(self):
        original=self.export(); audit.anchor(self.archive,self.key,'independent-1',original)
        original['document']['records'][0]['digest']='f'*64
        with self.assertRaises(RuntimeError):
            audit.anchor(self.archive,self.key,'independent-1',original)
    def test_control_identity_is_renewed_with_exact_hostname_and_matching_key(self):
        stage=self.root/'control'; stage.mkdir(mode=0o700)
        config={'kind':'control','hostname':'operator-control'}
        cert.generate(config,stage); cert.validate(config,stage)
        with self.assertRaises(RuntimeError):
            cert.validate({'kind':'control','hostname':'wrong-control'},stage)
    def test_vm_rotates_ca_server_client_and_both_eku_purposes(self):
        stage=self.root/'vm'; stage.mkdir(mode=0o700)
        cert.generate({'kind':'vm'},stage); cert.validate({'kind':'vm'},stage)
        subprocess.run(['openssl','verify','-CAfile',str(stage/'ca.pem'),'-purpose','sslclient',str(stage/'server.pem')],capture_output=True,check=False)
        wrong=subprocess.run(['openssl','verify','-CAfile',str(stage/'ca.pem'),'-purpose','sslserver',str(stage/'client.pem')],capture_output=True)
        self.assertNotEqual(wrong.returncode,0)
    def test_nonroot_bind_secret_keeps_read_mode_only_under_private_parent(self):
        target=self.root/'control-key.pem'
        cert.atomic(target,b'original',0o444)
        self.assertEqual(cert.material_path({'kind':'control'},target),target)
        cert.atomic(target,b'candidate',0o444)
        self.assertEqual(target.read_bytes(),b'candidate')
        self.assertEqual(target.stat().st_mode&0o777,0o444)
        self.root.chmod(0o755)
        try:
            with self.assertRaises(RuntimeError):
                cert.material_path({'kind':'control'},target)
        finally:
            self.root.chmod(0o700)

if __name__=='__main__':
    unittest.main()
