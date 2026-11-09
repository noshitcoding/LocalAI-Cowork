"""Pure input and image cleanup checks; no Docker daemon or fixture is used."""
import json
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import qualify_candidate_lifecycle as qualification


SOURCE = 'sha256:' + '1' * 64
TARGET = 'sha256:' + '2' * 64
CHILD = 'sha256:' + '3' * 64
SCOPE = 'cowork-lifecycle-test-' + 'a' * 32


class QualificationInputTests(unittest.TestCase):
    def arguments(self, **overrides):
        values = {'server_image_id': SOURCE, 'transition_server_image_id': TARGET,
                  'management_server_image_id': None, 'source_migration': 52,
                  'target_migration': 54, 'management_migration': None}
        values.update(overrides)
        return SimpleNamespace(**values)

    def test_cross_binary_defaults_to_target_manager(self):
        selected = qualification.qualification_inputs(self.arguments())
        self.assertTrue(selected['cross_binary'])
        self.assertEqual(selected['source_reference'], SOURCE)
        self.assertEqual(selected['transition_reference'], TARGET)
        self.assertEqual(selected['manager_reference'], TARGET)
        self.assertEqual(selected['target_migration'], 54)
        self.assertEqual(selected['manager_migration'], 54)

    def test_explicit_target_manager_is_accepted(self):
        selected = qualification.qualification_inputs(self.arguments(
            management_server_image_id=TARGET, management_migration=54))
        self.assertEqual(selected['manager_reference'], TARGET)
        self.assertEqual(selected['manager_migration'], 54)

    def test_schema59_target_keeps_distinct_binary_and_manager_gates(self):
        selected = qualification.qualification_inputs(self.arguments(target_migration=59))
        self.assertEqual(selected['target_migration'], 59)
        self.assertEqual(selected['manager_migration'], 59)
        self.assertTrue(selected['cross_binary'])
        with self.assertRaisesRegex(AssertionError, 'distinct source/target'):
            qualification.qualification_inputs(self.arguments(target_migration=59, transition_server_image_id=SOURCE))
        with self.assertRaisesRegex(RuntimeError, 'unsupported_lifecycle_schema_transition'):
            qualification.qualification_inputs(self.arguments(target_migration=60))

    def test_same_binary_defaults_preserve_source_schema(self):
        selected = qualification.qualification_inputs(self.arguments(
            transition_server_image_id=None, target_migration=None))
        self.assertFalse(selected['cross_binary'])
        self.assertEqual(selected['transition_reference'], SOURCE)
        self.assertEqual(selected['manager_reference'], SOURCE)
        self.assertEqual(selected['target_migration'], 52)
        self.assertEqual(selected['manager_migration'], 52)

    def test_old_manager_image_or_schema_cannot_qualify_cross_binary(self):
        for overrides in [{'management_server_image_id': SOURCE},
                          {'management_migration': 52},
                          {'management_server_image_id': SOURCE, 'management_migration': 52}]:
            with self.subTest(overrides=overrides), self.assertRaisesRegex(
                    AssertionError, 'target manager and schema'):
                qualification.qualification_inputs(self.arguments(**overrides))

    def test_cross_binary_rejects_identical_image_ids(self):
        with self.assertRaisesRegex(AssertionError, 'distinct source/target'):
            qualification.qualification_inputs(self.arguments(transition_server_image_id=SOURCE))

    def test_zero_and_negative_migrations_are_not_defaulted(self):
        for field in ['source_migration', 'target_migration', 'management_migration']:
            for version in [0, -1]:
                with self.subTest(field=field, version=version), self.assertRaisesRegex(
                        AssertionError, 'positive migration'):
                    qualification.qualification_inputs(self.arguments(**{field: version}))

    def test_target_schema_cannot_precede_source(self):
        with self.assertRaisesRegex(AssertionError, 'target migration precedes source'):
            qualification.qualification_inputs(self.arguments(target_migration=51))

    def test_server_references_must_be_exact_local_image_ids(self):
        for field in ['server_image_id', 'transition_server_image_id', 'management_server_image_id']:
            for reference in ['cowork-server:latest', 'sha256:abc', '']:
                with self.subTest(field=field, reference=reference), self.assertRaisesRegex(
                        AssertionError, 'exact local server image IDs'):
                    qualification.qualification_inputs(self.arguments(**{field: reference}))

    def test_invalid_matrix_fails_before_docker_or_fixture_creation(self):
        with patch.object(qualification, 'docker') as docker, \
                patch.object(qualification.tempfile, 'mkdtemp') as mkdtemp, \
                patch.object(qualification.shutil, 'copyfile') as copyfile:
            with self.assertRaisesRegex(AssertionError, 'target manager and schema'):
                qualification.outer(self.arguments(management_server_image_id=SOURCE))
            docker.assert_not_called()
            mkdtemp.assert_not_called()
            copyfile.assert_not_called()


class QualificationImageCleanupTests(unittest.TestCase):
    def setUp(self):
        self.baseline = {'existing-container-' + str(index): SOURCE for index in range(39)}
        self.value = {'Id': CHILD, 'Config': {'Labels': {'cowork.acceptance': SCOPE}},
                      'RepoTags': [SCOPE + ':transition']}
        self.in_use = False
        self.removed = []

    def docker(self, *arguments, **kwargs):
        if arguments == ('image', 'ls', '--quiet', '--no-trunc', '--filter',
                         'label=cowork.acceptance=' + SCOPE):
            return CHILD
        if arguments == ('image', 'inspect', CHILD):
            return json.dumps([self.value])
        if arguments == ('ps', '--all', '--quiet', '--filter', 'ancestor=' + CHILD):
            return 'using-container' if self.in_use else ''
        if arguments == ('image', 'ls', '--quiet', '--filter', 'reference=' + SCOPE + ':source'):
            return TARGET
        if arguments[:2] == ('image', 'rm'):
            self.removed.append(arguments[2:])
            return ''
        self.fail('unexpected Docker operation: ' + repr(arguments))

    def image(self, reference):
        self.assertEqual(reference, SCOPE + ':source')
        return reference, TARGET

    def cleanup(self, alias_base_image_id=TARGET):
        with patch.object(qualification, 'docker', side_effect=self.docker), \
                patch.object(qualification, 'image', side_effect=self.image):
            qualification.remove_fixture_images(SCOPE, self.baseline, alias_base_image_id)

    def test_target_base_alias_is_removed_without_removing_either_base(self):
        self.cleanup()
        self.assertEqual(self.removed, [(SCOPE + ':transition',), (SCOPE + ':source',)])
        self.assertFalse(any(identifier in args for args in self.removed
                             for identifier in [SOURCE, TARGET, CHILD]))
        self.assertEqual(set(self.baseline.values()), {SOURCE})
        self.assertEqual(len(self.baseline), 39)

    def test_source_id_cannot_authorize_target_base_alias_removal(self):
        with self.assertRaises(AssertionError):
            self.cleanup(alias_base_image_id=SOURCE)
        self.assertEqual(self.removed, [(SCOPE + ':transition',)])

    def test_foreign_tag_prevents_any_image_removal(self):
        self.value['RepoTags'].append('preexisting:stable')
        with self.assertRaisesRegex(AssertionError, 'foreign image reference'):
            self.cleanup()
        self.assertEqual(self.removed, [])

    def test_used_image_prevents_any_image_removal(self):
        self.in_use = True
        with self.assertRaisesRegex(AssertionError, 'fixture image still in use'):
            self.cleanup()
        self.assertEqual(self.removed, [])

    def test_baseline_image_prevents_any_image_removal(self):
        self.baseline['protected-container'] = CHILD
        with self.assertRaises(AssertionError):
            self.cleanup()
        self.assertEqual(self.removed, [])


if __name__ == '__main__':
    unittest.main()
