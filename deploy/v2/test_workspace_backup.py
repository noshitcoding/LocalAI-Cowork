"""Small private fixtures; no Docker, Kubernetes, database or live directory IO."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path,PurePosixPath
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import workspace_backup as workspaces

DEPLOYMENT='a'*32


class WorkspaceBackupTests(unittest.TestCase):
    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory(prefix='cowork-workspace-fixture-')
        self.base=Path(self.temporary.name).resolve()
        self.root=self.base/'source';self.root.mkdir(mode=0o700)
        if os.name=='posix' and os.getuid()!=1000:
            self.owner_patch=patch.object(workspaces,'owned',return_value={'workspace_id':'11111111-1111-4111-8111-111111111111'})
            self.owner_patch.start();self.addCleanup(self.owner_patch.stop)
        else:workspaces.owned(self.root,DEPLOYMENT,True)
        self.addCleanup(self.temporary.cleanup)

    def snapshot(self):
        (self.root/'empty').mkdir()
        (self.root/'source.txt').write_bytes(b'original workspace bytes')
        archive=self.base/'workspace.tar'
        workspaces.archive_workspace(self.root,archive,DEPLOYMENT,{'stopped_guests':[]})
        return archive

    def test_roundtrip_keeps_empty_directory_and_retains_newer_files(self):
        archive=self.snapshot()
        (self.root/'source.txt').write_bytes(b'newer workspace bytes')
        receipt=workspaces.restore_workspace(self.root,archive,DEPLOYMENT,DEPLOYMENT)
        self.assertTrue((self.root/'empty').is_dir())
        self.assertEqual((self.root/'source.txt').read_bytes(),b'original workspace bytes')
        self.assertEqual((self.root/receipt['retained_directory']/'source.txt').read_bytes(),b'newer workspace bytes')
        self.assertFalse(receipt['runs_replayed'])

    def test_changed_content_is_refused_before_retaining_or_overwriting(self):
        archive=self.snapshot()
        corrupt=self.base/'corrupt.tar'
        with tarfile.open(archive,'r:') as source,tarfile.open(corrupt,'w') as output:
            for member in source:
                raw=source.extractfile(member).read() if member.isfile() else None
                if member.name=='content/source.txt':raw=b'X'*len(raw)
                output.addfile(member,io.BytesIO(raw) if raw is not None else None)
        with self.assertRaises(ValueError):workspaces.restore_workspace(self.root,corrupt,DEPLOYMENT,DEPLOYMENT)
        self.assertEqual((self.root/'source.txt').read_bytes(),b'original workspace bytes')
        self.assertFalse(any(path.name.startswith('.cowork-retained') for path in self.root.iterdir()))

    def test_traversal_and_duplicate_members_are_refused(self):
        for names in [['content/../../outside'],['content/a','content/a']]:
            archive=self.base/('invalid-'+str(len(list(self.base.iterdir())))+'.tar')
            with tarfile.open(archive,'w') as output:
                for name in names:
                    item=tarfile.TarInfo(name);item.size=1;output.addfile(item,io.BytesIO(b'x'))
            with self.assertRaises(ValueError):workspaces.validate_archive(archive)
        self.assertFalse((self.base/'outside').exists())

    def test_hardlink_is_refused_without_reading_or_deleting_other_file(self):
        outside=self.base/'outside';outside.write_bytes(b'preserve')
        os.link(outside,self.root/'aliased')
        with self.assertRaises(ValueError):workspaces.archive_workspace(self.root,self.base/'unsafe.tar',DEPLOYMENT,{})
        self.assertEqual(outside.read_bytes(),b'preserve')

    def test_owned_guest_is_stopped_by_full_id_and_record_is_preserved(self):
        data=self.base/'data';control=data/'team-containers';control.mkdir(parents=True)
        host={'format':1,'host_id':'55555555-5555-4555-8555-555555555555','engine_id':'engine-1','node_name':None,'node_uid':None}
        (control/'hosts').mkdir();(control/'hosts'/('host-'+host['host_id']+'.json')).write_text(json.dumps(host))
        record={'format':2,'host':host,'nonce':'11111111-1111-4111-8111-111111111111','team':'22222222-2222-4222-8222-222222222222',
                'order':'33333333-3333-4333-8333-333333333333','run':'44444444-4444-4444-8444-444444444444',
                'name':'cowork-team-11111111-1111-4111-8111-111111111111','image':'tools@sha256:'+'a'*64,'container_id':'b'*64}
        path=control/('ct-'+record['nonce']+'.json');path.write_text(json.dumps(record));original=path.read_bytes()
        state={'Running':True,'Pid':123,'Restarting':False,'Paused':False}
        item={'id':record['container_id'],'name':'/'+record['name'],'image':record['image'],
              'labels':{'cowork.team-tool.'+key:record[key] for key in ['nonce','team','order','run','image']},
              'mounts':[{'Source':'/srv/workspaces/'+record['order'],'Destination':'/workspace','Type':'bind','RW':True}],'state':state}
        item['labels'].update({'cowork.team-tool.host-id':host['host_id'],'cowork.team-tool.engine-id':host['engine_id']})
        calls=[]
        def stop(*args):
            if args[0]=='info':return b'"engine-1"'
            calls.append(args);state.update(Running=False,Pid=0);return b''
        with patch.object(workspaces,'inspections',side_effect=lambda:[copy.deepcopy(item)]),patch.object(workspaces,'docker',side_effect=stop):
            receipt=workspaces.quiesce(self.root,data,DEPLOYMENT,PurePosixPath('/srv/workspaces'),host_id=host['host_id'])
        self.assertEqual(calls,[('stop','--time','45','b'*64)])
        self.assertEqual(path.read_bytes(),original)
        self.assertEqual(receipt['stopped_guests'][0]['run'],record['run'])

    def test_unrecorded_writer_is_retained_and_not_stopped(self):
        data=self.base/'data';data.mkdir()
        item={'id':'a'*64,'labels':{},'mounts':[{'Source':'/srv/workspaces','RW':True}],
              'state':{'Running':True,'Pid':1,'Restarting':False,'Paused':False}}
        with patch.object(workspaces,'inspections',return_value=[item]),patch.object(workspaces,'docker',return_value=b'"engine-1"') as docker:
            with self.assertRaises(ValueError):workspaces.quiesce(self.root,data,DEPLOYMENT,PurePosixPath('/srv/workspaces'),host_id='55555555-5555-4555-8555-555555555555')
            docker.assert_called_once_with('info','--format','{{json .ID}}')

    def test_foreign_host_and_unbound_legacy_records_never_trigger_local_stop(self):
        data=self.base/'data';control=data/'team-containers';(control/'hosts').mkdir(parents=True)
        host={'format':1,'host_id':'55555555-5555-4555-8555-555555555555','engine_id':'engine-1','node_name':None,'node_uid':None}
        (control/'hosts'/('host-'+host['host_id']+'.json')).write_text(json.dumps(host))
        record={'format':2,'host':host,'nonce':'11111111-1111-4111-8111-111111111111','team':'22222222-2222-4222-8222-222222222222',
            'order':'33333333-3333-4333-8333-333333333333','run':'44444444-4444-4444-8444-444444444444',
            'name':'cowork-team-11111111-1111-4111-8111-111111111111','image':'tools@sha256:'+'a'*64,'container_id':'b'*64}
        path=control/('ct-'+record['nonce']+'.json');path.write_text(json.dumps(record));before=path.read_bytes()
        with patch.object(workspaces,'docker',return_value=b'"engine-1"') as docker:
            with self.assertRaises(ValueError):workspaces.quiesce(self.root,data,DEPLOYMENT,host_id='66666666-6666-4666-8666-666666666666')
            docker.assert_called_once_with('info','--format','{{json .ID}}')
        self.assertEqual(path.read_bytes(),before)
        record.pop('host');record['format']=1;path.write_text(json.dumps(record));before=path.read_bytes()
        with patch.object(workspaces,'docker') as docker:
            with self.assertRaises(ValueError):workspaces.quiesce(self.root,data,DEPLOYMENT,host_id=host['host_id'])
            docker.assert_not_called()
        self.assertEqual(path.read_bytes(),before)


if __name__=='__main__':unittest.main()
