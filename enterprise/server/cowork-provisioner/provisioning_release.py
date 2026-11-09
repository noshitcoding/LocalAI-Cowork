"""Bind fresh provisioning to a catalog image set, independently of mutable defaults."""
import json
from release_catalog import ReleaseCatalog, ReleaseError, image_set_id, validate_release, DIGEST
from execution_generation import ExecutionGenerationError


def selection(operation):
    if operation.get('action') != 'provision':
        return None
    selected = operation['instance'].get('provisioning_release')
    arguments = operation.get('arguments', {})
    if (arguments.get('release') != selected
            or (selected is not None and (not isinstance(selected, str) or not DIGEST.fullmatch(selected)
                or type(arguments.get('provisioning_release_version')) is not int or arguments['provisioning_release_version'] != 1))
            or (selected is None and 'provisioning_release_version' in arguments)):
        raise ExecutionGenerationError('provisioning_release_binding_mismatch')
    return selected


def prepare(infrastructure, operation, directory, active):
    selected = selection(operation)
    if selected is None:
        return None
    instance = operation['instance']
    import backend_profile
    profiled=backend_profile.vm_instance(infrastructure.config,instance)
    if profiled:backend_profile.vm_scope(infrastructure,operation)
    if (infrastructure.config.get('release_selection_enabled') is not True
            or instance['backend'] != 'compose' or not (instance['policy']['minimum_isolation'] == 'container' or profiled)):
        raise ReleaseError('provisioning_release_profile_unavailable')
    catalog = ReleaseCatalog(infrastructure.config, infrastructure.private_write)
    if not catalog.enabled(selected):
        raise ReleaseError('release_withdrawn')
    # Creating an empty instance needs no source transition or update canary.
    release = validate_release(catalog.read(catalog.root / (selected + '.json')), selected)
    images = release['images']
    pinned = active / 'images.json'
    if pinned.exists() or pinned.is_symlink():
        if catalog.read(pinned) != images:
            raise ReleaseError('provisioning_release_pin_mismatch')
    if (directory / 'compose.json').exists() or (directory / 'compose.json').is_symlink():
        catalog.read(directory / 'compose.json')
        if infrastructure.deployed_images(directory) != images:
            raise ReleaseError('provisioning_release_manifest_mismatch')
    # Persist before secrets, database roles, buckets, or workloads are created.
    infrastructure.private_write(pinned, json.dumps(images, sort_keys=True).encode())
    return images


def verify_installed(infrastructure, operation, directory):
    selected = selection(operation)
    if selected is None:
        return
    active = infrastructure.active_directory(directory)
    pinned = active / 'images.json'
    if (pinned.is_symlink() or not pinned.is_file()
            or image_set_id(json.loads(pinned.read_text())) != selected
            or image_set_id(infrastructure.deployed_images(directory)) != selected):
        raise ExecutionGenerationError('provisioning_release_completion_unproven')


def attach(infrastructure, operation, directory, receipt):
    selected = selection(operation)
    if selected is not None:
        verify_installed(infrastructure, operation, directory)
        receipt.update(instance_id=operation['instance']['id'], release=selected)
    return receipt


def validate_receipt(operation, receipt, succeeded=True):
    selected = selection(operation)
    if not succeeded or operation.get('action') != 'provision':
        return
    if selected is None:
        if receipt.get('release') is not None:
            raise ExecutionGenerationError('unexpected_provisioning_release_receipt')
    elif (receipt.get('receipt_id') != operation['id'] or receipt.get('instance_id') != operation['instance']['id']
            or receipt.get('release') != selected):
        raise ExecutionGenerationError('provisioning_release_receipt_mismatch')
