"""Bind reviewed installed QA to unchanged signed candidate bytes in a new artifact.

Infrastructure helper only: no key generation, installation, or publication.
Run it from the independently reviewed default-branch checkout.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/release'))
import preview
import preview_trust
from release_checks import HOSTS, checked_package, ordinary, pin, read

QA_WORKFLOW = '.github/workflows/preview-acceptance.yml'


def producer_context(value: dict, source: dict, workflow: str) -> None:
    # Values enter through live_producer/fetch; keep their checked identity intact
    # when composing the two independently fetched original artifacts.
    if (value.get('head_sha') != source['commit'] or value.get('producer_workflow') != workflow
            or not isinstance(value.get('run_id'), int) or value['run_id'] <= 0
            or not isinstance(value.get('artifact_id'), int) or value['artifact_id'] <= 0
            or not re.fullmatch(r'sha256:[0-9a-f]{64}', value.get('artifact_digest') or '')):
        raise ValueError('Exact independently checked producer context is required')


def copy(original: Path, target: Path, expected: dict) -> None:
    if target.exists() or target.is_symlink() or pin(original) != expected:
        raise ValueError('Original evidence changed or the new output already exists')
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(original, target)
    if pin(original) != expected or pin(target) != expected:
        raise ValueError('Original evidence changed during copying')


def packages(candidate: Path, source: dict, trusted: dict) -> dict:
    if read(candidate / 'source-provenance.json') != source:
        raise ValueError('Original candidate does not bind this exact tagged source')
    manifest = read(candidate / 'candidate.json')
    if manifest.get('format') != 'localai-cowork-preview-candidate/1' or set(manifest.get('packages', {})) != set(HOSTS):
        raise ValueError('A complete original native candidate is required')
    result = {}
    for host, relative in manifest['packages'].items():
        folder = preview.selected(candidate, relative, directory=True)
        receipt, _ = checked_package(folder, source['version'], strict_preview=True, trusted_policy=trusted)
        if receipt['host'] != host:
            raise ValueError('Original candidate platform differs from its signed package')
        result[host] = {'folder': folder, 'receipt': receipt}
    return result


def evidence(qa: Path, acceptance: dict) -> dict:
    records = {}
    for platform in acceptance['platforms'].values():
        for check in platform['checks'].values():
            name = check['evidence']
            if not isinstance(name, str) or not name.startswith('evidence/'):
                raise ValueError('Select original QA logs only below evidence/')
            path = preview.selected(qa, name)
            record = pin(path)
            if record['size'] > 16 * 1024 * 1024:
                raise ValueError('Split oversized original QA logs into a bounded reviewed bundle')
            records[name] = record
    return records


def collect(candidate: Path, qa: Path, output: Path, source: dict, trusted: dict, producer: dict) -> None:
    producer_context(producer, source, trusted['candidate_workflow'])
    native = packages(candidate, source, trusted)
    acceptance_path = ordinary(qa, directory=True) / 'acceptance.json'
    acceptance = read(acceptance_path)
    original_acceptance = pin(acceptance_path)
    preview.checked_acceptance(qa, acceptance, source, native)
    logs = evidence(qa, acceptance)
    if output.exists() or output.is_symlink():
        raise ValueError('Select a new QA artifact output')
    output.mkdir(parents=True)
    copy(acceptance_path, output / 'acceptance.json', original_acceptance)
    for name, record in logs.items():
        copy(preview.selected(qa, name), output / name, record)
    binding = {'format': 'localai-cowork-preview-qa-binding/1', 'commit': source['commit'],
               'source_sha256': source['source_sha256'], 'candidate': producer,
               'acceptance_sha256': original_acceptance['sha256'], 'evidence': logs}
    (output / 'CANDIDATE-BINDING.json').write_text(json.dumps(binding, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    preview.checked_acceptance(output, read(output / 'acceptance.json'), source, native)


def finalize(candidate: Path, qa: Path, output: Path, source: dict, trusted: dict,
             producer: dict, qa_producer: dict, original_files: dict) -> None:
    producer_context(producer, source, trusted['candidate_workflow'])
    producer_context(qa_producer, source, QA_WORKFLOW)
    native = packages(candidate, source, trusted)
    binding = read(qa / 'CANDIDATE-BINDING.json')
    if (binding.get('format') != 'localai-cowork-preview-qa-binding/1'
            or binding.get('candidate') != producer or binding.get('commit') != source['commit']
            or binding.get('source_sha256') != source['source_sha256']
            or qa_producer.get('producer_workflow') != QA_WORKFLOW
            or binding.get('acceptance_sha256') != pin(qa / 'acceptance.json')['sha256']):
        raise ValueError('QA evidence does not bind the exact original candidate artifact')
    acceptance = read(qa / 'acceptance.json')
    preview.checked_acceptance(qa, acceptance, source, native)
    logs = evidence(qa, acceptance)
    if binding.get('evidence') != logs:
        raise ValueError('Original QA evidence changed after its authorized collection')
    if output.exists() or output.is_symlink() or (candidate / 'QA-FINALIZATION.json').exists():
        raise ValueError('Select a new output and the original build candidate')
    manifest = read(candidate / 'candidate.json')
    acceptance_name = manifest['acceptance']
    if acceptance_name != 'acceptance.json':
        raise ValueError('The original candidate must use the fixed acceptance entry')
    output.mkdir(parents=True)
    for name, record in original_files.items():
        if name != acceptance_name:
            copy(preview.selected(candidate, name), output / name, record)
    copy(qa / 'acceptance.json', output / acceptance_name, pin(qa / 'acceptance.json'))
    for name, record in logs.items():
        copy(preview.selected(qa, name), output / name, record)
    final_native = packages(output, source, trusted)
    preview.checked_acceptance(output, read(output / acceptance_name), source, final_native)
    # All original native receipts, packages, signatures, SBOMs and frontend evidence
    # retain their exact original bytes. Only pending acceptance is replaced.
    for name, record in original_files.items():
        if pin(preview.selected(candidate, name)) != record or (name != acceptance_name and pin(output / name) != record):
            raise ValueError('An original candidate input changed during finalization')
    proof = {'format': 'localai-cowork-preview-qa-finalization/1', 'commit': source['commit'],
             'source_sha256': source['source_sha256'], 'original_candidate': producer,
             'qa_producer': qa_producer, 'qa_binding_sha256': pin(qa / 'CANDIDATE-BINDING.json')['sha256'],
             'acceptance_sha256': pin(output / acceptance_name)['sha256'],
             'original_native_receipts_unchanged': True, 'release_qualified': False, 'published': False,
             'created_at': datetime.now(timezone.utc).isoformat()}
    (output / 'QA-FINALIZATION.json').write_text(json.dumps(proof, indent=2, sort_keys=True) + '\n', encoding='utf-8')


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['collect', 'finalize'])
    for name in ('source-root', 'trusted-policy', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    for name in ('tag', 'repository'):
        parser.add_argument('--' + name, required=True)
    for name in ('candidate-run-id', 'candidate-artifact-id'):
        parser.add_argument('--' + name, type=int, required=True)
    parser.add_argument('--qa-dir', type=Path)
    parser.add_argument('--qa-run-id', type=int)
    parser.add_argument('--qa-artifact-id', type=int)
    args = parser.parse_args()
    if (os.environ.get('GITHUB_REF_TYPE') != 'tag' or os.environ.get('GITHUB_REF_NAME') != args.tag
            or os.environ.get('GITHUB_REPOSITORY') != args.repository):
        raise ValueError('Run only in the explicitly authorized tag workflow context')
    source = preview.source_snapshot(args.source_root, args.tag)
    if source['commit'] != os.environ.get('GITHUB_SHA'):
        raise ValueError('Workflow context and tagged source commit differ')
    trusted = preview_trust.policy(args.trusted_policy, args.repository)
    if trusted['candidate_workflow'] != '.github/workflows/preview-candidate.yml':
        raise ValueError('The independent policy must authorize the candidate producer')
    if not args.output.is_absolute():
        raise ValueError('Select a new absolute output')
    scratch = args.output.parent / (args.output.name + '-original-inputs')
    scratch.mkdir()
    candidate = scratch / 'candidate'
    archive = scratch / 'candidate.zip'
    producer = preview_trust.fetch(args.source_root, trusted, source['commit'], args.candidate_run_id,
                                   args.candidate_artifact_id, archive, candidate)
    original_files = preview_trust.checked_archive(archive, candidate, producer)
    if args.action == 'collect':
        if args.qa_dir is None:
            raise ValueError('Select an existing explicit original QA evidence directory')
        collect(candidate, args.qa_dir, args.output, source, trusted, producer)
    else:
        if args.qa_run_id is None or args.qa_artifact_id is None:
            raise ValueError('Select the exact authorized QA run and immutable artifact IDs')
        qa_policy = dict(trusted, candidate_workflow=QA_WORKFLOW)
        qa = scratch / 'qa'
        qa_archive = scratch / 'qa.zip'
        qa_producer = preview_trust.fetch(args.source_root, qa_policy, source['commit'], args.qa_run_id,
                                          args.qa_artifact_id, qa_archive, qa)
        finalize(candidate, qa, args.output, source, trusted, producer, qa_producer, original_files)
        preview_trust.checked_archive(qa_archive, qa, qa_producer)
    preview_trust.checked_archive(archive, candidate, producer)
    print(json.dumps({'output': str(args.output), 'installed_evidence_validated': True,
                      'published': False, 'release_qualified': False}))


if __name__ == '__main__':
    main()
