"""Local operator origin adapter only: no network, signing, installation or execution.
The caller supplies an independently selected SHA256 of the original operator index.
Original commands/plans/raw bytes remain the evidence; no GitHub identities are minted.
"""
from pathlib import Path
import re
from verify_review import require,ordinary,pin,read_json,HOSTS
GROUPS=('core','desktop','release-tools','documents','office','document-ui')
PG=('postgres_same_storage_contract','postgres_environment_contract','postgres_process_contract',
 'postgres_mcp_contract','postgres_mcp_connection_contract','postgres_mcp_credential_contract',
 'postgres_native_key_setup_contract','postgres_workflow_contract','journal_cases::postgres_workflow_journal_contract',
 'postgres_document_draft_contract')
ACTOR=('postgres_original_creator_scope_fences_queue_and_existing_lease_without_other_changes',
 'postgres_separate_desktop_viewer_and_controller_keep_creator_and_original_rows')
REQUIRED_JOBS={p+'-'+g for p in ('windows','linux') for g in GROUPS}|{'linux-postgres:'+x for x in PG}|{'linux-device-actor-postgres:'+x for x in ACTOR}
def ref(record):
    require(isinstance(record,dict) and isinstance(record.get('path'),str),'Original file reference required')
    path=ordinary(Path(record['path']))
    require(pin(path)=={k:record.get(k) for k in ('size','sha256')},'Original referenced bytes changed')
    return path
def source(value,commit,tree):
    require(value.get('source_commit')==commit and value.get('source_tree')==tree,'Local receipt source mismatch')
def scope(value,argvs,host_os=None):
    records=value.get('owned_command_records')
    require(isinstance(records,list) and len(records)==len(argvs),'Original owned command records missing')
    for record,argv in zip(records,argvs):
        require(record.get('command')==argv and type(record.get('exit_code')) is int
            and record['exit_code']==0 and record.get('retained_popen_wait') is True,'Actual command/exit mismatch')
        require(record.get('native_get_exit_confirmed') is True or
            record.get('retained_pidfd_completion') is True and record.get('own_session_empty') is True,'Retained kernel completion missing')
    host_os=host_os or value.get('host_os')
    require(host_os in ('nt','posix'),'Recorded native host required')
    require(all(r.get('native_get_exit_confirmed') is True for r in records) if host_os=='nt' else
        all(r.get('retained_pidfd_completion') is True and r.get('own_session_empty') is True for r in records),
        'Native completion differs from selected host')
    process=value.get('native_process_scope',value.get('process_scope',{}))
    require(process.get('forced_cleanup') is not True,'Forced cleanup cannot pass')
    require(process.get('own_job_empty') is True and process.get('active_after_wait')==0 if host_os=='nt' else
        process.get('separate_native_linux_sessions') is True and process.get('windows_launcher_kill_job_used') is False,
        'Owned process scope incomplete')
def checked_job(record,job_id,commit,tree):
    plan_path=ref(record['plan']);result_path=ref(record['result'])
    plan=read_json(plan_path);result=read_json(result_path)
    require(plan.get('format')=='localai-cowork-local-command-plan/1'
        and result.get('format')=='localai-cowork-local-command-result/1','Unsupported local job format')
    source(plan,commit,tree);source(result,commit,tree)
    require(plan.get('host_os')==result.get('host_os') and plan.get('host_os') in ('nt','posix'),'Job native-host mismatch')
    if job_id.startswith('windows-') or job_id.endswith('-windows'):require(plan['host_os']=='nt','Windows gate needs native Windows evidence')
    if job_id.startswith('linux-') or job_id.endswith('-linux'):require(plan['host_os']=='posix','Linux gate needs native Linux evidence')
    require(plan.get('job_id')==job_id==result.get('job_id') and result.get('plan')==pin(plan_path),'Job/plan binding mismatch')
    require(result.get('result')=='passed' and result.get('source_before_equal') is True
        and result.get('source_after_equal') is True and result.get('source_before_after_equal') is True
        and result.get('source_manifest')==plan.get('source_manifest'),'Actual source/job gate not passed')
    steps=plan.get('steps',[]);observed=result.get('command_results',[])
    require(steps and len(steps)==len(observed),'Complete original command list required')
    ids=[s.get('id') for s in steps]
    require(len(ids)==len(set(ids)) and all(isinstance(x,str) and re.fullmatch(r'[a-z0-9][a-z0-9_.-]{0,99}',x) for x in ids),'Unique portable step IDs required')
    for step,actual in zip(steps,observed):
        for key in ('id','argv','cwd','executable','timeout_seconds'):
            require(actual.get(key)==step.get(key),'Actual command differs from reviewed plan')
        require(actual.get('environment',{})==step.get('environment',{}),'Actual command environment differs from reviewed plan')
        require(actual.get('private_environment_refs',{})==step.get('private_environment',{}),'Private input reference mismatch')
        require(actual.get('result')=='passed','A command did not pass')
        expected_names={step['id']+'.stdout.log',step['id']+'.stderr.log'}
        streams=actual.get('original_streams',{})
        require(set(streams)==expected_names,'Original stdout/stderr pair required')
        for name,original in streams.items():
            require(pin(result_path.parent/name)==original and original['size']<=64*1024**2,'Original stream bytes changed')
    scope(result,[s['argv'] for s in steps])
    return result
def checked_secrets(record,commit,tree):
    plan_path=ref(record['plan']);result_path=ref(record['result'])
    plan=read_json(plan_path);result=read_json(result_path)
    source(plan,commit,tree);source(result,commit,tree)
    require(plan.get('format')=='localai-cowork-local-secrets-readiness/1'
        and result.get('format')=='localai-cowork-local-secrets-result/1'
        and result.get('readiness')==pin(plan_path),'Original secret plan binding mismatch')
    require(result.get('result')=='passed' and result.get('finding_count')==0
        and result.get('source_before_after_equal') is True,'Local secret gate incomplete')
    commands=result.get('commands',[]);require(len(commands)==3,'Three original secret commands required')
    tool=plan['official_downloads']['extracted_only_exact_regular_gitleaks_exe']['path']
    expected=[plan['plan']['commands'][0],[tool,'version'],[
        x.replace('<new-private-evidence-root>/results.sarif',str(result_path.parent/'results.sarif'))
        for x in plan['plan']['commands'][1]]]
    # The reviewed plan uses --report-path=<placeholder>; only the output location varies.
    require([c.get('argv') for c in commands]==expected and all(c.get('result')=='passed' for c in commands),'Secret command/scope mismatch')
    for command in commands:
        require(set(command.get('raws',{}))=={'stdout','stderr'},'Original secret streams required')
        for stream,original in command['raws'].items():
            require(pin(result_path.parent/(command['name']+'.'+stream+'.log'))==original,'Original secret stream changed')
    report_path=result_path.parent/'results.sarif'
    require(pin(report_path)==result.get('sarif'),'Original SARIF changed')
    report=read_json(report_path)
    require(report.get('version')=='2.1.0' and report.get('runs')
        and sum(len(run.get('results',[])) for run in report['runs'])==0,'Original SARIF is not empty')
    scope(result,expected,host_os='nt')

def command_contract(step):
    return {key:step.get(key,{}) if key in ('environment','private_environment') else step.get(key)
        for key in ('argv','cwd','environment','private_environment','executable','timeout_seconds')}

def original_phase(record,commit,tree):
    require(isinstance(record,dict),'Original phase references required')
    plan=read_json(ref(record['plan']))
    result=checked_job(record,plan.get('job_id'),commit,tree)
    return plan,result

def checked_role(record,role,commit,tree):
    require(isinstance(record,dict),'Explicit role-to-original-phases mapping required')
    required=record.get('required_commands')
    phases=record.get('phases')
    require(isinstance(required,list) and required and len(required)<=512,'Independently reviewed complete role command recipe required')
    require(isinstance(phases,list) and phases and len(phases)<=128,'Original role phases required')
    require(all(isinstance(x,dict) and set(x)==set(command_contract({})) and isinstance(x.get('argv'),list) and x['argv']
        for x in required),'Complete explicit required command contracts needed')
    actual=[];seen=set()
    expected_os='nt' if role.startswith('windows-') else 'posix'
    for phase in phases:
        plan,result=original_phase(phase,commit,tree)
        require(plan.get('host_os')==expected_os,'Logical role/native platform mismatch')
        steps={step['id']:step for step in plan['steps']}
        ids=phase.get('step_ids')
        require(isinstance(ids,list) and ids and len(ids)==len(set(ids)) and set(ids)<=set(steps),'Explicit original role step selection required')
        for step_id in ids:
            identity=(phase['result']['sha256'],step_id)
            require(identity not in seen,'The same original step cannot cover a role twice')
            seen.add(identity);actual.append(command_contract(steps[step_id]))
    # The expected recipe is independent input, never inferred from whatever ran.
    # Counter equality preserves repeated required commands while allowing phases
    # to finish in a different order and keeping original IDs/receipts unchanged.
    from collections import Counter
    import json
    def counts(items):
        return Counter(json.dumps(x,sort_keys=True,separators=(',',':')) for x in items)
    require(counts(actual)==counts(required),'Incomplete or extra role command coverage')
    return {'role':role,'original_phase_count':len(phases),'covered_command_count':len(actual)}

def checked_selection(path,expected_sha256,commit,tree,platform):
    path=ordinary(path);require(pin(path)['sha256']==expected_sha256,'Independent operator selection digest mismatch')
    selection=read_json(path)
    require(selection.get('format')=='localai-cowork-local-origin-selection/1','Unsupported local origin selection')
    source(selection,commit,tree)
    require(platform in HOSTS and selection.get('platform')==platform,'Local producer platform mismatch')
    jobs=selection.get('verification_jobs',{})
    require(set(jobs)==REQUIRED_JOBS,'All24 exact Windows/Linux source jobs are required')
    for role,record in jobs.items():checked_role(record,role,commit,tree)
    checked_secrets(selection['secrets'],commit,tree)
    producer_plan,producer=original_phase(selection['producer'],commit,tree)
    common_plan,common=original_phase(selection['common_frontend'],commit,tree)
    require(producer_plan.get('host_os')==('nt' if platform=='windows' else 'posix'),'Original producer platform mismatch')
    require(selection.get('common_frontend_receipt_sha256') and re.fullmatch(r'[0-9a-f]{64}',selection['common_frontend_receipt_sha256']),'Common frontend receipt digest required')
    archive=ref(selection['archive'])
    require(0<archive.stat().st_size<=4*1024**3,'Original archive bound exceeded')
    return {'selection':selection,'selection_sha256':expected_sha256,'producer':producer,'common':common,
        'verification':{'format':'localai-cowork-local-verification/1','source_commit':commit,'source_tree':tree,
            'operator_selection_sha256':expected_sha256,'roles':sorted(jobs),'secrets':'passed'},
        'archive':pin(archive)}
def checked_origin(review,provenance,selection,context):
    original=context['selection']
    source(selection,original['source_commit'],original['source_tree'])
    require(selection.get('platform')==original.get('platform'),'Selected local platform differs from operator origin')
    require(review.get('format')=='localai-cowork-local-unsigned-build-review/1'
        and provenance.get('format')=='localai-cowork-local-unsigned-review-source/1','Local origin format required')
    require(review.get('verification')==provenance.get('verification')==context['verification'],'Original local verification binding mismatch')
    require(review.get('local_producer_receipt_sha256')==original['producer']['result']['sha256'],'Original local producer receipt mismatch')
    require(review.get('frontend_receipt_sha256')==original['common_frontend_receipt_sha256'],'Original common frontend identity mismatch')
    require(context['archive']['sha256']==selection.get('archive_sha256'),'Local archive differs from independently selected original')

