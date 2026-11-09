"""Durable completion delivery; a lost process never grants permission to replay."""
import json
import uuid
import execution_generation
import execution_recovery
import instance_enrollment
import provisioning_release
import executor_host
import host_capacity_attestation
import backend_profile
from contextlib import contextmanager
from pathlib import Path


class JournalError(Exception):
    pass


_operator_descriptor = None


def operator_pass_fds():
    return () if _operator_descriptor is None else (_operator_descriptor,)


@contextmanager
def operator_lock(root: Path):
    # A PID file or an expired lease is not proof that the old executor exited.
    import fcntl
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = root / "operator.lock"
    if path.is_symlink():
        raise JournalError("unsafe_operator_lock")
    with path.open("a+b") as descriptor:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise JournalError("another_operator_owns_state_directory") from error
        global _operator_descriptor
        _operator_descriptor = descriptor.fileno()
        try:
            yield
        finally:
            _operator_descriptor = None
        # Do not explicitly LOCK_UN: any surviving infrastructure child keeps
        # this same lock until it exits, including after a parent process crash.


def canonical_id(value):
    if str(uuid.UUID(value)) != value:
        raise JournalError("noncanonical_operation_identifier")
    return value


def binding(operation):
    provisioning_release.selection(operation)
    result = {name: operation[name] for name in ["id", "organisation_id", "instance_id", "action", "arguments", "request_digest", "lease_token"]}
    if operation['action'].startswith('import_'):
        import instance_imports
        result['instance_import_binding'] = instance_imports.binding(operation)
    for name in ["id", "organisation_id", "instance_id", "lease_token"]:
        canonical_id(result[name])
    if any(operation["instance"][key] != result[value] for key, value in [("id", "instance_id"), ("organisation_id", "organisation_id")]):
        raise JournalError("operation_scope_mismatch")
    generation = execution_generation.binding(operation)
    if generation is not None:
        result['execution_binding'] = generation
    if operation.get('execution_runtime_required') is True:
        result['execution_runtime_required'] = True
    recovery=execution_recovery.binding(operation)
    if recovery is not None:
        result['execution_recovery_binding']=recovery
    host = executor_host.operation_binding(operation)
    if host is not None:
        result['executor_host_binding'] = host
    if operation.get('backend_profile') is not None:
        result['backend_profile']=operation['backend_profile']
    allocation = host_capacity_attestation.operation_allocation(operation)
    if allocation is not None:
        result['host_allocation'] = allocation
    return result


class OperationJournal:
    def __init__(self, infrastructure, operation):
        self.infrastructure, self.operation = infrastructure, operation
        self.binding = binding(operation)
        directory = infrastructure.directory(operation["instance_id"])
        if hasattr(infrastructure, 'validate_host_operation'):
            infrastructure.validate_host_operation(operation, directory)
        compact = uuid.UUID(operation["id"]).hex
        self.path = directory / ("operation-" + compact + ".json")
        self.legacy_receipt = directory / ("receipt-" + compact + ".json")

    def read(self, path):
        if path.is_symlink():
            raise JournalError("unsafe_operation_journal")
        if not path.exists():
            return None
        try:
            return json.loads(path.read_text())
        except (ValueError, OSError) as error:
            raise JournalError("invalid_operation_journal") from error

    def state(self):
        state = self.read(self.path)
        if state is not None and (not isinstance(state, dict) or state.get("version") != 1 or state.get("binding") != self.binding):
            raise JournalError("operation_journal_binding_mismatch")
        return state

    def write(self, completion=None):
        self.infrastructure.private_write(self.path, json.dumps({"version": 1, "binding": self.binding, "completion": completion}).encode())

    def validate_completion(self, completion):
        if not isinstance(completion, dict) or completion.get("lease_token") != self.binding["lease_token"] or type(completion.get("succeeded")) is not bool:
            raise JournalError("invalid_operation_completion")
        result = completion.get("result")
        if self.operation['action'].startswith('import_'):
            import instance_imports
            instance_imports.validate_receipt(self.operation, result)
        if not isinstance(result, dict) or (completion["succeeded"] and result.get("receipt_id") != self.binding["id"]):
            raise JournalError("operation_receipt_mismatch")
        execution_generation.validate_receipt(self.operation,result)
        execution_recovery.validate_receipt(self.operation,result)
        instance_enrollment.validate_receipt(self.operation,result)
        provisioning_release.validate_receipt(self.operation,result,completion["succeeded"])
        executor_host.validate_receipt(self.operation,result)
        backend_profile.validate_receipt(self.infrastructure.config,self.operation,result)
        host_capacity_attestation.validate_receipt(self.operation,result)
        if self.operation['action']=='enroll_execution' and completion['succeeded']!=(result['execution_enrollment']['outcome']=='enrolled'):
            raise JournalError('execution_enrollment_outcome_mismatch')
        return completion

    def recover(self):
        state = self.state()
        if state is not None and state.get("completion") is not None:
            return self.validate_completion(state["completion"])
        # Compatibility with receipts written before journals, and the narrow
        # crash window between infrastructure completion and journal completion.
        receipt = self.read(self.legacy_receipt)
        if receipt is not None:
            completion = self.validate_completion({"lease_token": self.binding["lease_token"], "succeeded": True, "result": receipt})
            self.write(completion)
            return completion
        return None

    def begin(self):
        if self.state() is not None:
            raise JournalError("operation_started_without_completion")
        self.write()

    def finish(self, succeeded, result):
        if 'execution' not in result:
            result = execution_generation.attach(self.infrastructure,self.operation,result,succeeded)
        result = executor_host.attach_receipt(self.operation,result)
        result = host_capacity_attestation.attach_receipt(self.operation,result)
        result = backend_profile.attach(self.operation,result)
        completion = self.validate_completion({"lease_token": self.binding["lease_token"], "succeeded": succeeded, "result": result})
        self.write(completion)
        return completion
