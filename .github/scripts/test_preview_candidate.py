"""Synthetic QA lineage counterexamples, never installed-product qualification."""
import json
from pathlib import Path
import shutil
import sys
import unittest

import preview_candidate as infrastructure

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'tools/release'))
import test_preview as fixtures
from release_checks import pin, read


class QaLineageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fixtures.PreviewTests.setUpClass()

    def setUp(self):
        self.fixture = fixtures.PreviewTests('test_stage_curates_public_assets_and_draft_cannot_become_latest')
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        f = self.fixture
        self.qa = f.root / 'original-qa'
        (self.qa / 'evidence').mkdir(parents=True)
        acceptance = read(f.candidate / 'acceptance.json')
        shutil.copyfile(f.candidate / 'acceptance.log', self.qa / 'evidence/product.log')
        for platform in acceptance['platforms'].values():
            for record in platform['checks'].values():
                record['evidence'] = 'evidence/product.log'
        fixtures.write(self.qa / 'acceptance.json', acceptance)
        pending = read(f.candidate / 'acceptance.json')
        for platform in pending['platforms'].values():
            for record in platform['checks'].values():
                record['result'] = 'pending'
        fixtures.write(f.candidate / 'acceptance.json', pending)
        self.original = {p.relative_to(f.candidate).as_posix(): pin(p) for p in f.candidate.rglob('*') if p.is_file()}
        self.producer = {'run_id': 7, 'artifact_id': 11, 'artifact_digest': 'sha256:' + 'a' * 64,
                         'producer_workflow': f.trusted['candidate_workflow'], 'head_sha': f.source['commit']}
        self.qa_producer = {'run_id': 8, 'artifact_id': 12, 'artifact_digest': 'sha256:' + 'b' * 64,
                            'producer_workflow': infrastructure.QA_WORKFLOW, 'head_sha': f.source['commit']}
        self.collected = f.root / 'collected-qa'
        self.final = f.root / 'final-candidate'

    def collect(self):
        f = self.fixture
        infrastructure.collect(f.candidate, self.qa, self.collected, f.source, f.trusted, self.producer)

    def finalize(self):
        f = self.fixture
        infrastructure.finalize(f.candidate, self.collected, self.final, f.source, f.trusted,
                                self.producer, self.qa_producer, self.original)

    def test_new_artifact_preserves_original_receipts_and_every_native_byte(self):
        self.collect()
        self.finalize()
        for name, record in self.original.items():
            self.assertEqual(pin(self.fixture.candidate / name), record)
            if name != 'acceptance.json':
                self.assertEqual(pin(self.final / name), record)
        self.assertEqual(pin(self.final / 'acceptance.json'), pin(self.qa / 'acceptance.json'))
        proof = read(self.final / 'QA-FINALIZATION.json')
        self.assertEqual(proof['original_candidate']['artifact_id'], 11)
        self.assertEqual(proof['qa_producer']['artifact_id'], 12)
        self.assertFalse(proof['release_qualified'])
        self.assertFalse(proof['published'])

    def test_qa_for_another_original_artifact_is_refused(self):
        self.collect()
        binding = read(self.collected / 'CANDIDATE-BINDING.json')
        binding['candidate']['artifact_id'] = 99
        fixtures.write(self.collected / 'CANDIDATE-BINDING.json', binding)
        with self.assertRaisesRegex(ValueError, 'exact original candidate'):
            self.finalize()
        self.assertFalse(self.final.exists())

    def test_changed_original_package_receipt_cannot_reuse_acceptance(self):
        self.collect()
        folder = next(iter(self.fixture.packages.values()))
        receipt = read(folder / 'receipt.json')
        receipt['unrelated_new_field'] = 'changes original receipt bytes'
        fixtures.write(folder / 'receipt.json', receipt)
        with self.assertRaisesRegex(ValueError, 'different package receipt'):
            self.finalize()

    def test_changed_qa_log_is_refused(self):
        self.collect()
        (self.collected / 'evidence/product.log').write_text('replaced')
        with self.assertRaisesRegex(ValueError, 'evidence changed'):
            self.finalize()

    def test_pending_checks_cannot_be_collected_as_success(self):
        acceptance = read(self.qa / 'acceptance.json')
        next(iter(acceptance['platforms'].values()))['checks']['model_connection']['result'] = 'pending'
        fixtures.write(self.qa / 'acceptance.json', acceptance)
        with self.assertRaisesRegex(ValueError, 'not accepted'):
            self.collect()
        self.assertFalse(self.collected.exists())

    def test_collection_rejects_other_user_files_outside_evidence(self):
        acceptance = read(self.qa / 'acceptance.json')
        shutil.copyfile(self.qa / 'evidence/product.log', self.qa / 'outside.log')
        for platform in acceptance['platforms'].values():
            for record in platform['checks'].values():
                record['evidence'] = 'outside.log'
        fixtures.write(self.qa / 'acceptance.json', acceptance)
        with self.assertRaisesRegex(ValueError, 'only below evidence'):
            self.collect()

    def test_unapproved_qa_producer_cannot_finalize(self):
        self.collect()
        self.qa_producer['producer_workflow'] = '.github/workflows/unapproved.yml'
        with self.assertRaisesRegex(ValueError, 'producer context'):
            self.finalize()

    def test_qa_run_from_another_commit_is_refused(self):
        self.collect()
        self.qa_producer['head_sha'] = 'f' * 40
        with self.assertRaisesRegex(ValueError, 'producer context'):
            self.finalize()

    def test_acceptance_from_another_source_is_refused(self):
        acceptance = read(self.qa / 'acceptance.json')
        acceptance['source_sha256'] = 'f' * 64
        fixtures.write(self.qa / 'acceptance.json', acceptance)
        with self.assertRaises(ValueError): self.collect()
        self.assertFalse(self.collected.exists())

    def test_acceptance_from_another_package_set_is_refused(self):
        acceptance = read(self.qa / 'acceptance.json')
        platform = next(iter(acceptance['platforms'].values()))
        platform['packages'][next(iter(platform['packages']))] = 'f' * 64
        fixtures.write(self.qa / 'acceptance.json', acceptance)
        with self.assertRaises(ValueError): self.collect()
        self.assertFalse(self.collected.exists())

    def test_replayed_candidate_digest_is_refused(self):
        self.collect()
        self.producer['artifact_digest'] = 'sha256:' + 'f' * 64
        with self.assertRaisesRegex(ValueError, 'exact original candidate'):
            self.finalize()

    def test_replayed_candidate_run_is_refused(self):
        self.collect()
        self.producer['run_id'] = 99
        with self.assertRaisesRegex(ValueError, 'exact original candidate'):
            self.finalize()

    def test_mutated_native_bundle_is_refused(self):
        self.collect()
        folder = next(iter(self.fixture.packages.values()))
        receipt = read(folder / 'receipt.json')
        bundle = next(iter(receipt['bundles'].values()))
        (folder / bundle['filename']).write_bytes(b'changed original bundle')
        with self.assertRaises(ValueError): self.finalize()

    def test_mutated_sbom_is_refused(self):
        self.collect()
        folder = next(iter(self.fixture.packages.values()))
        (folder / 'sbom.spdx.json').write_text('{}')
        with self.assertRaises(ValueError): self.finalize()

    def test_evidence_cannot_override_acceptance_or_native_receipts(self):
        acceptance = read(self.qa / 'acceptance.json')
        for name in ('acceptance.json', 'windows/receipt.json'):
            with self.subTest(name=name):
                for platform in acceptance['platforms'].values():
                    for record in platform['checks'].values(): record['evidence'] = name
                with self.assertRaisesRegex(ValueError, 'only below evidence'):
                    infrastructure.evidence(self.qa, acceptance)

    def test_existing_candidate_output_is_preserved(self):
        self.collect()
        self.final.mkdir()
        marker = self.final / 'keep'
        marker.write_text('existing output')
        with self.assertRaisesRegex(ValueError, 'new output'):
            self.finalize()
        self.assertEqual(marker.read_text(), 'existing output')


if __name__ == '__main__':
    unittest.main()
