"""Synthetic transport-counterexample tests only; no compilation, networking or package execution."""
from pathlib import Path
import copy,json,shutil,tempfile,unittest
from unittest.mock import patch
import local_origin as local
import verify_review as verify
HERE=Path(__file__).resolve().parent
COMMIT='a'*40;TREE='b'*40
class LocalOrigin(unittest.TestCase):
    def setUp(self):
        base=HERE/'test-work';base.mkdir(exist_ok=True)
        self.root=Path(tempfile.mkdtemp(prefix='origin-test-',dir=base)).resolve()
        self.base=base.resolve()
    def tearDown(self):
        assert self.root.parent==self.base and self.root.name.startswith('origin-test-')
        shutil.rmtree(self.root)
    def save(self,name,value):
        p=self.root/name
        p.write_text(json.dumps(value),encoding='utf-8');return p
    def ref(self,p):return {'path':str(p),**verify.pin(p)}
    def job(self):
        step={'id':'one','argv':['synthetic-python','-m','unittest'],'cwd':'.','environment':{},
            'executable':{'size':1,'sha256':'c'*64},'timeout_seconds':600}
        plan={'format':'localai-cowork-local-command-plan/1','host_os':'nt','source_commit':COMMIT,
            'source_tree':TREE,'source_manifest':{'size':1,'sha256':'d'*64},'job_id':'windows-release-tools','steps':[step]}
        pp=self.save('plan.json',plan)
        streams={}
        for name,data in [('one.stdout.log',b'Ran 1 test; OK\n'),('one.stderr.log',b'')]:
            p=self.root/name;p.write_bytes(data);streams[name]=verify.pin(p)
        result={'format':'localai-cowork-local-command-result/1','host_os':'nt','job_id':plan['job_id'],
            'source_commit':COMMIT,'source_tree':TREE,'plan':verify.pin(pp),'source_manifest':plan['source_manifest'],
            'result':'passed','source_before_equal':True,'source_after_equal':True,'source_before_after_equal':True,
            'command_results':[{**step,'result':'passed','private_environment_refs':{},'original_streams':streams}],
            'owned_command_records':[{'command':step['argv'],'exit_code':0,'retained_popen_wait':True,'native_get_exit_confirmed':True}],
            'native_process_scope':{'own_job_empty':True,'active_after_wait':0,'forced_cleanup':False}}
        rp=self.save('result.json',result)
        return {'plan':self.ref(pp),'result':self.ref(rp)},plan,result
    def test_exact_original_job_passes_without_claiming_release(self):
        record,_,_=self.job()
        result=local.checked_job(record,'windows-release-tools',COMMIT,TREE)
        self.assertEqual(result['result'],'passed')
        self.assertNotIn('release_qualified',result)
    def test_raw_byte_change_rejected_even_if_result_and_plan_are_untouched(self):
        record,_,_=self.job();(self.root/'one.stdout.log').write_bytes(b'changed output')
        with self.assertRaisesRegex(ValueError,'stream bytes'):local.checked_job(record,'windows-release-tools',COMMIT,TREE)
    def test_command_different_from_reviewed_plan_rejected(self):
        record,_,result=self.job();result['command_results'][0]['argv']=['synthetic-python','-c','pass']
        rp=self.save('changed-result.json',result);record['result']=self.ref(rp)
        with self.assertRaisesRegex(ValueError,'reviewed plan'):local.checked_job(record,'windows-release-tools',COMMIT,TREE)
    def test_wrong_source_and_missing_native_completion_rejected(self):
        record,_,result=self.job()
        with self.assertRaisesRegex(ValueError,'source mismatch'):local.checked_job(record,'windows-release-tools','e'*40,TREE)
        result['owned_command_records'][0]['native_get_exit_confirmed']=False
        rp=self.save('bad-native.json',result);record['result']=self.ref(rp)
        with self.assertRaisesRegex(ValueError,'kernel completion'):local.checked_job(record,'windows-release-tools',COMMIT,TREE)
    def test_changed_plan_cannot_keep_old_plan_binding(self):
        record,plan,_=self.job();plan['steps'][0]['timeout_seconds']=1
        pp=self.save('changed-plan.json',plan);record['plan']=self.ref(pp)
        with self.assertRaisesRegex(ValueError,'plan binding'):local.checked_job(record,'windows-release-tools',COMMIT,TREE)
    def test_full_selection_rejects_missing_matrix_and_wrong_independent_pin(self):
        p=self.save('selection.json',{'format':'localai-cowork-local-origin-selection/1',
            'source_commit':COMMIT,'source_tree':TREE,'platform':'windows','verification_jobs':{}})
        with self.assertRaisesRegex(ValueError,'selection digest'):local.checked_selection(p,'0'*64,COMMIT,TREE,'windows')
        with self.assertRaisesRegex(ValueError,'All24'):local.checked_selection(p,verify.pin(p)['sha256'],COMMIT,TREE,'windows')
    def test_hosted_wrapper_keeps_original_hosted_identity_requirements(self):
        review={'format':'localai-cowork-unsigned-build-review/1','run_id':7,'run_attempt':1,
            'verification':{'head_sha':COMMIT,'path':'.github/workflows/verify.yml','conclusion':'success'}}
        provenance={'format':'localai-cowork-unsigned-review-source/1'}
        def material(root,src,selection,origin):origin(review,provenance);return 'material-checks-called'
        with patch.object(verify,'_checked_material_tree',side_effect=material):
            self.assertEqual(verify.checked_tree(None,None,{'source_commit':COMMIT},{'id':7,'run_attempt':1}),'material-checks-called')
            review['verification']['path']='forged-local-status'
            with self.assertRaisesRegex(ValueError,'hosted'):verify.checked_tree(None,None,{'source_commit':COMMIT},{'id':7,'run_attempt':1})

    def test_local_origin_accepts_only_selected_original_producer_and_archive(self):
        original={'source_commit':COMMIT,'source_tree':TREE,'platform':'windows',
            'producer':{'result':{'sha256':'1'*64}},'common_frontend_receipt_sha256':'2'*64}
        verification={'format':'localai-cowork-local-verification/1','source_commit':COMMIT,'source_tree':TREE}
        context={'selection':original,'verification':verification,'archive':{'sha256':'3'*64}}
        selection={'source_commit':COMMIT,'source_tree':TREE,'platform':'windows','archive_sha256':'3'*64}
        review={'format':'localai-cowork-local-unsigned-build-review/1','verification':verification,
            'local_producer_receipt_sha256':'1'*64,'frontend_receipt_sha256':'2'*64}
        provenance={'format':'localai-cowork-local-unsigned-review-source/1','verification':verification}
        local.checked_origin(review,provenance,selection,context)
        bad=copy.deepcopy(review);bad['local_producer_receipt_sha256']='4'*64
        with self.assertRaisesRegex(ValueError,'producer receipt'):local.checked_origin(bad,provenance,selection,context)
        wrong=copy.deepcopy(selection);wrong['archive_sha256']='5'*64
        with self.assertRaisesRegex(ValueError,'archive'):local.checked_origin(review,provenance,wrong,context)
    def test_local_receipts_cannot_be_relabelled_as_other_source_or_platform(self):
        original={'source_commit':COMMIT,'source_tree':TREE,'platform':'windows'}
        context={'selection':original}
        wrong={'source_commit':'e'*40,'source_tree':TREE,'platform':'windows'}
        with self.assertRaisesRegex(ValueError,'source mismatch'):local.checked_origin({}, {},wrong,context)
        wrong={'source_commit':COMMIT,'source_tree':TREE,'platform':'linux'}
        with self.assertRaisesRegex(ValueError,'platform'):local.checked_origin({}, {},wrong,context)


    def phase(self,folder,job_id,argument):
        original_root=self.root
        self.root=original_root/folder;self.root.mkdir()
        record,plan,result=self.job()
        plan['job_id']=result['job_id']=job_id
        plan['steps'][0]['argv']=['synthetic-python',argument]
        result['command_results'][0]['argv']=plan['steps'][0]['argv']
        result['owned_command_records'][0]['command']=plan['steps'][0]['argv']
        pp=self.save('alias-plan.json',plan);result['plan']=verify.pin(pp)
        rp=self.save('alias-result.json',result)
        phase={'plan':self.ref(pp),'result':self.ref(rp),'step_ids':['one']}
        self.root=original_root
        return phase,local.command_contract(plan['steps'][0])
    def test_original_alias_job_id_is_preserved_while_role_is_canonical(self):
        phase,expected=self.phase('alias','windows-release-tools-source74','tools')
        role={'required_commands':[expected],'phases':[phase]}
        before=Path(phase['result']['path']).read_bytes()
        self.assertEqual(local.checked_role(role,'windows-release-tools',COMMIT,TREE)['covered_command_count'],1)
        self.assertEqual(Path(phase['result']['path']).read_bytes(),before)
    def test_split_role_requires_all_predeclared_commands(self):
        rust,a=self.phase('rust','windows-core-rust-source74','rust')
        front,b=self.phase('front','windows-core-frontend-source74','frontend')
        role={'required_commands':[a,b],'phases':[rust,front]}
        self.assertEqual(local.checked_role(role,'windows-core',COMMIT,TREE)['original_phase_count'],2)
        role['phases']=[rust]
        with self.assertRaisesRegex(ValueError,'coverage'):local.checked_role(role,'windows-core',COMMIT,TREE)
    def test_role_cannot_count_same_original_step_twice_or_infer_coverage(self):
        phase,a=self.phase('dup','windows-core-rust-source74','rust')
        with self.assertRaisesRegex(ValueError,'twice'):
            local.checked_role({'required_commands':[a,a],'phases':[phase,phase]},'windows-core',COMMIT,TREE)
        with self.assertRaisesRegex(ValueError,'complete role command recipe'):
            local.checked_role({'required_commands':[],'phases':[phase]},'windows-core',COMMIT,TREE)
    def test_wrong_native_platform_cannot_fill_role(self):
        phase,a=self.phase('os','windows-release-tools-source74','tools')
        with self.assertRaisesRegex(ValueError,'platform'):
            local.checked_role({'required_commands':[a],'phases':[phase]},'linux-release-tools',COMMIT,TREE)

if __name__=='__main__':unittest.main()

