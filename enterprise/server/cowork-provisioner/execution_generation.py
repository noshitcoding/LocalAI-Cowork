"""Scoped execution epochs for staged resources and durable lifecycle receipts.

This binds metadata and generated runtime configuration. Fleet-wide activation
and qualification of the actual admission-enabled server images are separate.
"""
import json
import hashlib
import uuid


class ExecutionGenerationError(Exception):
    pass


def check(value, code):
    if not value:
        raise ExecutionGenerationError(code)


def identifier(value):
    try:
        check(type(value) is str and str(uuid.UUID(value)) == value, 'invalid_execution_epoch_identifier')
    except (ValueError, AttributeError, TypeError):
        raise ExecutionGenerationError('invalid_execution_epoch_identifier') from None
    return value


def binding(operation):
    value = operation.get('execution_binding')
    if value is None:
        return None
    fields = {'operation_id', 'organisation_id', 'instance_id', 'source_epoch', 'candidate_epoch'}
    check(isinstance(value, dict) and set(value) == fields, 'invalid_execution_generation_binding')
    for field in fields:
        identifier(value[field])
    check(value['operation_id'] == operation['id'] and value['instance_id'] == operation['instance']['id']
          and value['organisation_id'] == operation['instance']['organisation_id'], 'execution_generation_scope_mismatch')
    rotates = operation['action'] in ['restore', 'upgrade', 'rollback', 'import_restore']
    check((value['candidate_epoch'] != value['source_epoch']) == rotates, 'execution_generation_transition_mismatch')
    return value


def document(path, limit=4096):
    try:
        check(not path.is_symlink() and path.is_file() and path.stat().st_nlink == 1
              and path.stat().st_size <= limit, 'unsafe_execution_generation_document')
        value = json.loads(path.read_text(encoding='utf-8'))
    except (ValueError, OSError):
        raise ExecutionGenerationError('invalid_execution_generation_document') from None
    check(isinstance(value,dict), 'invalid_execution_generation_document')
    return value


def read(instance, active):
    path = active / 'execution.json'
    check(not path.is_symlink(), 'unsafe_execution_generation_marker')
    if not path.exists():
        return None
    value = document(path)
    check(isinstance(value, dict) and set(value) == {'version', 'instance_id', 'organisation_id', 'epoch'}
          and type(value['version']) is int and value['version'] == 1, 'invalid_execution_generation_marker')
    check(value['instance_id'] == instance['id'] and value['organisation_id'] == instance['organisation_id'], 'execution_generation_scope_mismatch')
    return identifier(value['epoch'])


def write(infrastructure, instance, active, epoch):
    prior = read(instance, active)
    check(prior is None or prior == epoch, 'execution_generation_marker_is_immutable')
    infrastructure.private_write(active / 'execution.json', json.dumps({
        'version': 1, 'instance_id': instance['id'], 'organisation_id': instance['organisation_id'], 'epoch': identifier(epoch),
    }, sort_keys=True).encode())


def prepare(infrastructure, operation, directory):
    value = binding(operation)
    instance = operation['instance']
    active = infrastructure.active_directory(directory)
    prior = read(instance, active)
    if operation['action']=='enroll_execution':
        check(value is not None and operation.get('execution_runtime_required') is True,'execution_enrollment_binding_required')
        return # Only the enrollment transaction can create a legacy marker.
    if value is None:
        check(prior is None, 'execution_generation_binding_required')
        transitional=infrastructure.config.get('execution_enrollment_enabled') is True and operation['action'] in ['stop','suspend','backup','upgrade','delete','import_quiesce','import_export']
        check(infrastructure.config.get('execution_admission') is None or transitional, 'execution_generation_binding_required')
        return
    if instance.get('backend')!='compose':
        import backend_profile
        selected=backend_profile.operation(infrastructure.config,operation)
        check(instance.get('backend')=='kubernetes' and selected is not None and selected['specification']['kind']=='kubernetes','execution_admission_backend_not_qualified')
    check(instance.get('execution_epoch') == value['source_epoch'], 'execution_generation_source_changed')
    if prior is None:
        # Never silently enroll an old running fleet or rewrite retained source
        # evidence while attempting an update. Legacy enrollment is a separate gate.
        check(operation['action'] == 'provision' and active == directory
              and not (directory / 'compose.json').exists(), 'execution_generation_enrollment_required')
        write(infrastructure, instance, active, value['source_epoch'])
    else:
        check(prior == value['source_epoch'], 'execution_generation_source_changed')


def stage(infrastructure, operation, directory):
    value = binding(operation)
    if value is not None:
        write(infrastructure, operation['instance'], directory, value['candidate_epoch'])


def attach(infrastructure, operation, result, succeeded):
    value = binding(operation)
    if value is None:
        check('execution' not in result, 'unexpected_execution_generation_receipt')
        return result
    directory = infrastructure.directory(operation['instance']['id'])
    runtime = None
    if operation['action'] == 'delete' and succeeded and result.get('deletion_verified') is True:
        # Deleted infrastructure has no active runtime. Keep its last fence; this
        # receipt never releases reservations of an offline device.
        deleted = directory / 'deleted.json'
        check(document(deleted) == result,
              'execution_generation_deletion_unproven')
        epoch = value['source_epoch']
    else:
        active = infrastructure.active_directory(directory)
        epoch = read(operation['instance'], active)
        check(epoch in [value['source_epoch'], value['candidate_epoch']], 'execution_generation_active_unproven')
        manifest = directory / 'compose.json'
        check(not manifest.is_symlink(), 'unsafe_execution_generation_manifest')
        if succeeded or manifest.exists():
            contents = document(manifest,1048576)
            # Import here to keep generation parsing independent of Docker probes.
            from execution_runtime import validate_manifest
            validate_manifest(infrastructure.config, contents)
            services = contents.get('services')
            check(isinstance(services,dict), 'execution_generation_manifest_mismatch')
            for service in ['api', 'worker']:
                entry = services.get(service)
                check(isinstance(entry,dict), 'execution_generation_manifest_mismatch')
                env = entry.get('environment')
                check(isinstance(env,dict) and env.get('COWORK_EXECUTION_EPOCH') == epoch
                      and env.get('COWORK_INSTANCE_ID') == value['instance_id']
                      and env.get('COWORK_ORGANISATION_ID') == value['organisation_id'], 'execution_generation_manifest_mismatch')
            if operation.get('execution_runtime_required') is True:
                from execution_runtime import probe
                probe(infrastructure, operation['instance'], active, {'server': services['api']['image']})
                runtime = {'version': 1, 'server_image': services['api']['image'],
                           'manifest_sha256': hashlib.sha256(json.dumps(contents,sort_keys=True,separators=(',',':')).encode()).hexdigest()}
    receipt = {'binding': value, 'active_epoch': epoch}
    if runtime is not None: receipt['runtime'] = runtime
    return {**result, 'execution': receipt}


def validate_receipt(operation, result):
    value = binding(operation)
    receipt = result.get('execution')
    if value is None:
        check(receipt is None, 'unexpected_execution_generation_receipt')
        return
    check(isinstance(receipt, dict) and set(receipt) in [{'binding', 'active_epoch'}, {'binding', 'active_epoch', 'runtime'}] and receipt['binding'] == value,
          'execution_generation_receipt_mismatch')
    check(receipt['active_epoch'] in [value['source_epoch'], value['candidate_epoch']], 'execution_generation_receipt_mismatch')
    runtime=receipt.get('runtime')
    if operation.get('execution_runtime_required') is True and operation['action']!='delete' and not result.get('error_code'):
        check(runtime is not None,'execution_runtime_attestation_required')
    if runtime is not None:
        import re
        check(isinstance(runtime,dict) and set(runtime)=={'version','server_image','manifest_sha256'}
              and type(runtime['version']) is int and runtime['version']==1
              and isinstance(runtime['server_image'],str) and re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[0-9a-f]{64}',runtime['server_image'])
              and isinstance(runtime['manifest_sha256'],str) and re.fullmatch('[a-f0-9]{64}',runtime['manifest_sha256']),
              'invalid_execution_runtime_attestation')
