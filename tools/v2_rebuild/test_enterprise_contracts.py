import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from export_enterprise_contracts import ROOTS, ROUTES, build, canonical, closure, pin, references, rewrite_references, sha, write_bundle
from check_enterprise_contracts import check


class EnterpriseContractExportTests(unittest.TestCase):
    def fixture(self, root):
        (root / 'contracts').mkdir()
        (root / 'package.json').write_bytes(canonical({'version': '2.0.0-alpha.test'}))
        definitions = {name: {'type': 'object', 'additionalProperties': False} for name in ROOTS}
        definitions['ExecutionPlan'] = {'type': 'object', 'properties': {'boundary': {'$ref': '#/$defs/Boundary'}}}
        definitions['Boundary'] = {'type': 'object', 'properties': {'child': {'$ref': '#/$defs/Boundary'}}}
        definitions['Unrelated'] = {'type': 'string'}
        schema = {'$schema': 'https://json-schema.org/draft/2020-12/schema', '$defs': definitions}
        paths = {name: {'post': {'responses': {'200': {'content': {'application/json': {'schema': {'$ref': '#/components/schemas/PreparedExecutionPlan'}}}}}}} for name in ROUTES}
        api = {'openapi': '3.1.0', 'info': {'title': 'Fixture', 'version': '2.0.0-alpha.test'}, 'servers': [{'url': '/api/v2'}],
               'security': [{'daemonBearer': []}], 'paths': paths,
               'components': {'schemas': rewrite_references(definitions, '#/$defs/', '#/components/schemas/'),
                              'securitySchemes': {'daemonBearer': {'type': 'http', 'scheme': 'bearer'}}}}
        (root / 'contracts/api.schema.json').write_bytes(canonical(schema))
        (root / 'contracts/openapi.json').write_bytes(canonical(api))
        return schema, api

    def test_actual_reference_closure_is_deterministic_without_unrelated_types(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); self.fixture(root)
            first = build(root); second = build(root)
            self.assertEqual(first, second)
            bundle_id, content, manifest = first
            self.assertEqual(bundle_id, sha(content['manifest.json']))
            definitions = json.loads(content['execution.schema.json'])['$defs']
            self.assertIn('Boundary', definitions); self.assertNotIn('Unrelated', definitions)
            self.assertEqual(set(references(definitions, '#/$defs/')), {'Boundary'})
            self.assertEqual(manifest['definition_count'], len(ROOTS) + 1)
            self.fixture_again(root)
            self.assertNotEqual(build(root)[0], bundle_id)

    def fixture_again(self, root):
        schema = json.loads((root/'contracts/api.schema.json').read_bytes())
        api = json.loads((root/'contracts/openapi.json').read_bytes())
        schema['$defs']['Boundary']['description'] = 'New contract semantics'
        api['components']['schemas'] = rewrite_references(schema['$defs'], '#/$defs/', '#/components/schemas/')
        (root/'contracts/api.schema.json').write_bytes(canonical(schema))
        (root/'contracts/openapi.json').write_bytes(canonical(api))
        return schema, api

    def test_missing_and_external_references_are_refused(self):
        for value in ({'$ref': '#/$defs/Missing'}, {'$ref': 'https://example.invalid/remote'}, {'$ref': '#/$defs/A/properties/B'}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                closure({'A': value}, {'A'})

    def test_divergent_generated_schema_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); _, api=self.fixture(root)
            api['components']['schemas']['Boundary']['type']='string'
            (root/'contracts/openapi.json').write_bytes(canonical(api))
            with self.assertRaisesRegex(ValueError, 'disagree'): build(root)

    def test_immutable_bundle_retries_preserve_bytes_and_refuse_tampering(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); self.fixture(root)
            bundle_id, content, _=build(root)
            target=write_bundle(root/'artifacts',bundle_id,content)
            self.assertEqual(write_bundle(root/'artifacts',bundle_id,content),target)
            (target/'execution.schema.json').write_text('{}')
            with self.assertRaisesRegex(ValueError, 'differs'): write_bundle(root/'artifacts',bundle_id,content)
            with self.assertRaises(ValueError): write_bundle(root/'artifacts','../escape',content)

    def test_enterprise_pin_is_explicit_and_does_not_overwrite_unrelated_source(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); self.fixture(root)
            bundle_id, content, _=build(root)
            server=root/'enterprise'; source=server/'server/cowork-server/src'; source.mkdir(parents=True)
            (source.parent/'Cargo.toml').write_text('[package]\nname = "cowork-server"\n')
            target=pin(server,bundle_id,content)
            before=target.read_bytes();self.assertEqual(pin(server,bundle_id,content).read_bytes(),before)
            self.assertIn(bundle_id.encode(),before)
            target.write_text('// unrelated work\n')
            with self.assertRaisesRegex(ValueError, 'unrelated'): pin(server,bundle_id,content)

    def test_transient_windows_publication_lock_retries_without_publishing_partial_files(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); self.fixture(root); bundle_id,content,_=build(root)
            rename=os.rename
            def locked_once(source,target):
                self.assertFalse(Path(target).exists())
                self.assertEqual({p.name for p in Path(source).iterdir()},set(content))
                if not getattr(locked_once,'attempted',False):
                    locked_once.attempted=True
                    raise PermissionError('simulated sync/indexer lock')
                return rename(source,target)
            with mock.patch('export_enterprise_contracts.os.rename',side_effect=locked_once) as move,mock.patch('export_enterprise_contracts.time.sleep') as delay:
                target=write_bundle(root/'artifacts',bundle_id,content)
            self.assertEqual(move.call_count,2);delay.assert_called_once_with(0.1)
            self.assertTrue(all((target/name).read_bytes()==value for name,value in content.items()))

    def test_permanent_publication_lock_remains_an_error_and_leaves_no_partial_bundle(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);self.fixture(root);bundle_id,content,_=build(root)
            with mock.patch('export_enterprise_contracts.os.rename',side_effect=PermissionError('persistent lock')) as move,mock.patch('export_enterprise_contracts.time.sleep'):
                with self.assertRaises(PermissionError):write_bundle(root/'artifacts',bundle_id,content)
            self.assertEqual(move.call_count,7);self.assertFalse((root/'artifacts'/bundle_id).exists())

    def test_normalization_changes_only_reference_fields(self):
        value={'$ref':'#/$defs/A','description':'#/$defs/A','items':[{'$ref':'#/$defs/B'}]}
        original=copy.deepcopy(value)
        rewritten=rewrite_references(value,'#/$defs/','#/components/schemas/')
        self.assertEqual(value,original)
        self.assertEqual(rewritten['description'],'#/$defs/A')
        self.assertEqual(references(rewritten,'#/components/schemas/'),{'A','B'})

    def test_claimed_bundle_id_does_not_authorize_changed_bytes(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); self.fixture(root)
            bundle_id, content, _=build(root)
            changed={**content,'openapi.json':b'{}\n'}
            with self.assertRaisesRegex(ValueError,'manifest'): write_bundle(root/'artifacts',bundle_id,changed)
            with self.assertRaisesRegex(ValueError,'manifest'): write_bundle(root/'artifacts','0'*64,content)
            self.assertFalse((root/'artifacts').exists())

    def test_current_check_separates_unrelated_provenance_from_a_breaking_contract(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); self.fixture(root)
            bundle_id, content, manifest=build(root)
            server=root/'enterprise'; source=server/'server/cowork-server/src'; source.mkdir(parents=True)
            (source.parent/'Cargo.toml').write_text('[package]\nname = "cowork-server"\n')
            pin(server,bundle_id,content)
            changed=copy.deepcopy(manifest); changed['sources']['unrelated.rs']='0'*64
            new_manifest=canonical(changed); same_contract={**content,'manifest.json':new_manifest}
            with mock.patch('check_enterprise_contracts.build',return_value=(sha(new_manifest),same_contract,changed)):
                result=check(server)
            self.assertTrue(result['common_execution_contracts_match_current'])
            self.assertFalse(result['same_producer_snapshot']);self.assertFalse(result['execution_qualified'])
            changed_contract={**same_contract,'execution.schema.json':b'{}\n'}
            with mock.patch('check_enterprise_contracts.build',return_value=(sha(new_manifest),changed_contract,changed)):
                with self.assertRaisesRegex(ValueError,'schema changed'):check(server)


if __name__ == '__main__': unittest.main()
