"""Fake-runner proof vectors and binding checks without starting any service."""
import json
import threading
from types import SimpleNamespace
import unittest

from qualify_runner_wait import RunnerWaitFixture, no_start_proof, request_signature


class RunnerWaitProofTests(unittest.TestCase):
    def test_no_start_matches_the_independent_rust_fixed_vector(self):
        proof = no_start_proof(b'k' * 32, 'POST', '/v1/jobs', '1700000000',
                              '00000000-0000-4000-8000-000000000001', b'{"run_id":"fixture"}')
        self.assertEqual(proof, 'v1:a97929d8a4c4b5a99db42ca778a568afcbdca1951c81878dcca50d7324262349')

    def test_no_start_is_separate_from_request_authentication(self):
        fields = ('POST', '/v1/jobs', '1700000000', 'request-nonce', b'{}')
        self.assertNotEqual(no_start_proof(b'k' * 32, *fields)[3:], request_signature(b'k' * 32, *fields))

    def test_every_request_binding_changes_no_start_proof(self):
        fields = ['POST', '/v1/jobs', '1700000000', 'request-nonce', b'{}']
        proof = no_start_proof(b'k' * 32, *fields)
        for index, replacement in enumerate(['GET', '/v2/jobs', '1700000001', 'different-nonce', b'{"other":1}']):
            changed = list(fields)
            changed[index] = replacement
            with self.subTest(field=index):
                self.assertNotEqual(no_start_proof(b'k' * 32, *changed), proof)
        self.assertNotEqual(no_start_proof(b'x' * 32, *fields), proof)

    def test_failure_diagnostics_exclude_lease_and_request_content(self):
        sensitive = 'private-sensitive-fixture-value'
        scenario = SimpleNamespace(key='owned', mode='success')
        fixture = RunnerWaitFixture.__new__(RunnerWaitFixture)
        fixture.fake_runner = SimpleNamespace(lock=threading.Lock(), scenarios={'owned': scenario}, errors=[],
                                             records=lambda selected: [{'body': sensitive, 'nonce': sensitive}])
        fixture.owned_runs = {'owned': 'owned-run'}
        fixture.api = fixture.worker = SimpleNamespace(poll=lambda: None)
        fixture.snapshot = lambda run_id: {'state': 'running', 'reason': 'runner_capacity',
            'owner': sensitive, 'token': sensitive, 'error_code': sensitive,
            'checkpoint': {'safe_to_resume': True, 'executor_state': {
                'phase': 'sandbox_waiting_capacity', 'argv': sensitive, 'result': sensitive}}}
        diagnostics = fixture.diagnostics()
        self.assertNotIn(sensitive, json.dumps(diagnostics))
        self.assertEqual(diagnostics['cases'], [{'case': 'success', 'request_count': 1, 'run_state': 'running',
            'waiting_reason': 'runner_capacity', 'checkpoint_phase': 'sandbox_waiting_capacity',
            'safe_to_resume': True, 'error_code': None}])


if __name__ == '__main__':
    unittest.main()
