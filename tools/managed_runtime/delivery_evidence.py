"""Export bounded original pre-GC frames; these private bytes mint no authority."""
import hashlib
import json
import os
from pathlib import Path
import stat


def read_private(path, maximum, exact=None):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as source:
        before = os.fstat(source.fileno())
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != 0 or before.st_nlink != 1
                or stat.S_IMODE(before.st_mode) != 0o600 or before.st_size > maximum
                or exact is not None and before.st_size != exact):
            raise ValueError('Original delivery evidence identity/bound changed: ' + path.name)
        data = source.read(maximum + 1)
        after = os.fstat(source.fileno())
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (
                after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns) or len(data) != before.st_size:
            raise ValueError('Original delivery evidence changed during read')
    return data


def frame(data, allow_empty=False):
    if not any(data) and allow_empty:
        return None
    length = int.from_bytes(data[:4], 'little')
    if (len(data) <= 36 or not 0 < length <= len(data) - 36
            or hashlib.sha256(data[36:36 + length]).digest() != data[4:36]
            or any(data[36 + length:])):
        raise ValueError('Original delivery frame hash/length/padding changed')
    return data[36:36 + length]


def export_delivery(mountpoint, output, report):
    controller = mountpoint / 'controller'
    exported = []
    for index in (1, 2):
        for stem in ('controller-delivery-ready', 'controller-delivery-retired',
                     'native-delivery-captured', 'native-delivery-collected'):
            path = controller / (stem + '-' + str(index) + '.json')
            if path.exists():
                data = read_private(path, 2 * 1024**2)
                (output / path.name).write_bytes(data)
        captured = controller / ('native-delivery-captured-' + str(index) + '.json')
        if not captured.exists():
            continue
        capture = json.loads(read_private(captured, 2 * 1024**2))
        if capture.get('format_version') != 1 or capture.get('index') != index or capture.get('diagnostic_only') is not True:
            raise ValueError('Original pre-delivery capture changed')
        candidates = []
        for name in ('task.a', 'task.b', 'outcome.bin'):
            path = controller / ('delivery-capture-' + str(index) + '-' + name + '.bin')
            size = 1024**2 if name == 'outcome.bin' else 16384
            data = read_private(path, size, exact=size)
            (output / path.name).write_bytes(data)
            payload = frame(data, allow_empty=name != 'outcome.bin')
            if name == 'outcome.bin':
                outcome = payload
            elif payload:
                candidates.append((json.loads(payload), payload))
        record, original_payload = max(candidates, key=lambda value: value[0]['sequence'])
        if (record != capture['physical_record']
                or hashlib.sha256(original_payload).hexdigest() != capture['physical_record_payload_sha256']
                or hashlib.sha256(json.dumps(record, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest() != capture['physical_record_sha256']
                or hashlib.sha256(outcome).hexdigest() != record['outcome_sha256']
                or json.loads(outcome) != capture['receipt']['payload']['state']['result']):
            raise ValueError('Original captured task, signed result and outcome differ')
        existing = [n for n, value in enumerate(report['task_records'])
                    if value['request_sha256'] == record['request_sha256']]
        if existing:
            if len(existing) != 1 or report['task_records'][existing[0]] != record:
                raise ValueError('Retained and captured original physical records differ')
        else:
            n = len(report['task_records'])
            report['task_records'].append(record)
            (output / ('task-record-' + str(n) + '.json')).write_text(json.dumps(record, indent=2) + '\n')
            (output / ('task-outcome-' + str(n) + '.json')).write_bytes(outcome)
        if any(value['operation'] == 'task_start' and value['acknowledged'] for value in record['mutations']):
            report['workload_tasks_launched'] = True
        collected = controller / ('native-delivery-collected-' + str(index) + '.json')
        exported.append({'index': index, 'physical_record_sha256': capture['physical_record_sha256'],
                         'collected': json.loads(read_private(collected, 2 * 1024**2)) if collected.exists() else None})
    if exported:
        report['original_delivery_captures'] = exported
    journal = mountpoint / 'service-state/managed-delivery-journal'
    if not journal.exists():
        return
    mode = journal.lstat()
    if not stat.S_ISDIR(mode.st_mode) or mode.st_uid != 0 or stat.S_IMODE(mode.st_mode) != 0o700:
        raise ValueError('Private original delivery journal directory changed')
    entries = {'header.bin': 512 * 1024, **{'slot-' + str(n).zfill(4) + '.bin': 32768 for n in range(8)}}
    # Exactly eight preallocated slots in this dedicated fixture, never secrets.
    if set(path.name for path in journal.iterdir()) != set(entries) | {'owner.lock'}:
        raise ValueError('Fixture delivery journal membership differs')
    hashes = {}
    for name, size in entries.items():
        data = read_private(journal / name, size, exact=size)
        destination = output / ('delivery-journal-' + name)
        destination.write_bytes(data)
        hashes[name] = hashlib.sha256(data).hexdigest()
    report['original_delivery_journal'] = {'diagnostic_only': True, 'files': hashes}


def validate_delivery(output, physical):
    """Bind actual Controller results to original physical frames and final slots."""
    canonical = lambda value: hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
    observation = json.loads((output / 'managed-service-observation.json').read_bytes())
    control = observation['controller']
    jobs = control.get('jobs', [])
    native = observation.get('native_delivery', [])
    records = physical.get('task_records', [])
    if (control.get('status') != 'passed' or control.get('delivery') is not True
            or control.get('both_local_results_readable_after_gc') is not True
            or len(jobs) != 2 or len(native) != 2 or len(records) != 2
            or len({job['run_id'] for job in jobs}) != 2
            or len({job['process_attempt_id'] for job in jobs}) != 2
            or len({job['result_version_id'] for job in jobs}) != 2):
        raise ValueError('Both original Controller plans, attempts and local Ready versions are required')
    slots = []
    journal_files = physical.get('original_delivery_journal', {}).get('files', {})
    for name, digest in journal_files.items():
        data = (output / ('delivery-journal-' + name)).read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError('Exported original delivery journal changed')
        if name.startswith('slot-'):
            accepted = frame(data[:16384], allow_empty=True)
            if accepted:
                status_frames = [frame(data[start:start + 8192], allow_empty=True) for start in (16384, 24576)]
                slots.append((json.loads(accepted), [json.loads(value) for value in status_frames if value]))
    if len(slots) != 2:
        raise ValueError('Exactly two original accepted fixed journal slots are required')
    diagnostic = (output / 'native-failure-slot.bin').read_bytes()
    if len(diagnostic) != 16384:
        raise ValueError('Native diagnostic slot size changed')
    frame(diagnostic, allow_empty=True)
    for index, (job, held) in enumerate(zip(jobs, native), 1):
        if job['index'] != index or held['index'] != index:
            raise ValueError('Original delivery order changed')
        receipt = held['receipt']
        matching = [value for value in records if value['request_sha256'] == receipt['payload']['request']['request_sha256']]
        if len(matching) != 1:
            raise ValueError('Original completed native job missing')
        record = matching[0]
        if (any(record.get(key) is not True for key in ('physical_stop_confirmed', 'control_process_reaped', 'output_complete', 'guest_exit_confirmed'))
                or len(record['mutations']) != 12 or any(not value['acknowledged'] or value['response'] is None for value in record['mutations'])
                or receipt['payload']['state']['state'] != 'completed'
                or receipt['payload']['state']['result']['termination'] != 'exited'
                or receipt['payload']['state']['result']['exit_code'] != 0
                or job['execution_receipt_sha256'] != canonical(receipt)
                or job['execution_receipt_sha256'] != held['execution_receipt_sha256']
                or job['manifest_sha256'] != held['manifest_sha256']
                or job['result_version_id'] != held['result_version_id']
                or held['original_namespaces_absent'] is not True or held.get('collected') is not True
                or held['pool_before']['reserved_bytes'] != 48 * 1024**2
                or held['pool_before']['reserved_volumes'] != 2
                or any(held['pool_after'][key] != 0 for key in ('reserved_bytes', 'reserved_volumes', 'retained_records'))):
            raise ValueError('Native completion, original result or quota evidence differs')
        delivery = held['delivery']
        status = held['retirement']
        original = delivery['payload']['request']['payload']
        if (original['execution_receipt_sha256'] != canonical(receipt)
                or original['recipient_version_id'] != job['result_version_id']
                or original['manifest_sha256'] != job['manifest_sha256']
                or status['payload']['state'] != 'retired'
                or status['payload']['delivery_receipt_sha256'] != canonical(delivery)
                or status['payload']['physical_completion_sha256'] != held['physical_record_sha256']):
            raise ValueError('Signed original delivery and retirement bindings differ')
        fixed = [value for value in slots if value[0].get('kind') == 'accepted' and value[0]['receipt'] == delivery]
        if len(fixed) != 1 or not fixed[0][1]:
            raise ValueError('Original accepted receipt/final evidence not durably retained')
        latest = max(fixed[0][1], key=lambda value: value['status']['payload']['revision'])
        if latest['status'] != status or canonical(latest['local_evidence']) != status['payload']['retirement_evidence_sha256']:
            raise ValueError('Private physical evidence differs from original final signed status')
