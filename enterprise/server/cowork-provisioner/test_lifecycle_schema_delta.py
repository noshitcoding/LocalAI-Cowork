"""Pure schema59 lifecycle preservation checks; no services or database starts."""
import copy
import json
from pathlib import Path
import re
import unittest

from lifecycle_schema_delta import LifecycleSchemaDelta, TABLES_BY_MIGRATION, TABLE_INVENTORY_SQL


class FixtureSql:
    def __init__(self, source):
        self.source = source
        self.current = source
        self.base = {'runs', 'run_checkpoints', 'projects', '_sqlx_migrations'} | {
            table for version, tables in TABLES_BY_MIGRATION.items() if version <= source for table in tables}
        self.rows = [{'id': 'same-run', 'spec': {'immutable': True}, 'result': {'content': 'preserved'}}]
        self.count_overrides = {}
        self.extra = set()
        self.missing = set()
        self.provenance_count = '0'
        self.calls = []

    def added(self):
        return {table for version, tables in TABLES_BY_MIGRATION.items()
                if self.source < version <= self.current for table in tables}

    def __call__(self, statement):
        self.calls.append(statement)
        if statement == TABLE_INVENTORY_SQL:
            return json.dumps(sorted((self.base | self.added() | self.extra) - self.missing))
        if statement.startswith('SELECT jsonb_build_object('):
            return json.dumps({table: self.count_overrides.get(table, 0) for table in self.added()})
        if statement.startswith('SELECT count(*) FROM run_checkpoints WHERE origin'):
            return self.provenance_count
        return json.dumps(self.rows)


QUERY = "SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY id),'[]'::jsonb) FROM runs r WHERE id IN ('own-id');"


class LifecycleSchemaDeltaTests(unittest.TestCase):
    def prepare(self, source=52, target=59):
        guard, sql = LifecycleSchemaDelta(source, target), FixtureSql(source)
        baseline = json.loads(guard.historical_runs(sql, QUERY, source))
        return guard, sql, baseline

    def transition(self, sql, target=59):
        sql.current = target
        if sql.source < 57 <= target:
            sql.rows[0].update(runner_cancel_id=None, runner_cancel_lease=None, runner_stop_receipt=None)
        if sql.source < 59 <= target:
            sql.rows[0]['server_resume_checkpoint_id'] = None

    def test_52_and_54_to_59_allow_only_actual_null_columns_and_empty_new_tables(self):
        for source in [52, 54]:
            with self.subTest(source=source):
                guard, sql, baseline = self.prepare(source)
                self.transition(sql)
                self.assertEqual(json.loads(guard.historical_runs(sql, QUERY, 59)), baseline)
                evidence = guard.evidence()
                self.assertEqual(evidence['target_schema'], 59)
                self.assertEqual(set(evidence['observations'][-1]['verified_empty_tables']), sql.added())
                self.assertIn('run_resume_requests', evidence['observations'][-1]['verified_empty_tables'])

    def test_nonnull_resume_or_cancel_fields_are_never_discarded(self):
        for name in ['server_resume_checkpoint_id', 'runner_cancel_id', 'runner_cancel_lease', 'runner_stop_receipt']:
            with self.subTest(name=name):
                guard, sql, _ = self.prepare(); self.transition(sql)
                sql.rows[0][name] = 'unexpected-real-data'
                with self.assertRaisesRegex(RuntimeError, 'historical_runs_migration_state_changed'):
                    guard.historical_runs(sql, QUERY, 59)

    def test_every_new_table_with_a_row_is_rejected_before_history_is_read(self):
        for table in {name for tables in TABLES_BY_MIGRATION.values() for name in tables}:
            with self.subTest(table=table):
                guard, sql, _ = self.prepare(); self.transition(sql)
                sql.count_overrides[table] = 1
                with self.assertRaisesRegex(RuntimeError, 'new_lifecycle_migration_table_not_empty'):
                    guard.historical_runs(sql, QUERY, 59)
                self.assertNotEqual(sql.calls[-1], QUERY)

    def test_missing_new_column_is_not_accepted_as_null(self):
        guard, sql, _ = self.prepare(); self.transition(sql)
        del sql.rows[0]['server_resume_checkpoint_id']
        with self.assertRaisesRegex(RuntimeError, 'historical_runs_migration_state_changed'):
            guard.historical_runs(sql, QUERY, 59)

    def test_unknown_new_fields_and_rows_remain_visible_to_the_exact_comparison(self):
        guard, sql, baseline = self.prepare(); self.transition(sql)
        sql.rows[0]['not_a_known_nullable_migration_column'] = None
        normalized = json.loads(guard.historical_runs(sql, QUERY, 59))
        self.assertNotEqual(normalized, baseline)
        self.assertIn('not_a_known_nullable_migration_column', normalized[0])
        added = copy.deepcopy(sql.rows[0]); added['id'] = 'new-history-row'; sql.rows.append(added)
        self.assertEqual(len(json.loads(guard.historical_runs(sql, QUERY, 59))), 2)

    def test_existing_schema59_values_are_preserved_without_normalization(self):
        guard, sql, _ = self.prepare(59, 59)
        sql.rows[0]['server_resume_checkpoint_id'] = 'existing-real-resume'
        sql.rows[0]['runner_cancel_id'] = 'existing-cancel'
        self.assertEqual(json.loads(guard.historical_runs(sql, QUERY, 59)), sql.rows)

    def test_changed_historical_checkpoint_provenance_is_rejected(self):
        guard, sql, _ = self.prepare(); self.transition(sql); sql.provenance_count = '1'
        with self.assertRaisesRegex(RuntimeError, 'historical_checkpoint_provenance_changed'):
            guard.historical_runs(sql, QUERY, 59)
        old = [{'id': 'old', 'executor_state': {'data': 'preserved'}}]
        migrated = [dict(old[0], origin='untrusted', worker_id=None,
            worker_lease_token=None, server_execution_attempt_id=None)]
        self.assertEqual(guard.normalize_rows('run_checkpoints', migrated, 59), old)
        migrated[0]['origin'] = 'server_agent_v1'
        with self.assertRaises(RuntimeError): guard.normalize_rows('run_checkpoints', migrated, 59)

    def test_missing_existing_or_unknown_new_tables_are_rejected(self):
        for change in ['missing', 'extra']:
            guard, sql, _ = self.prepare(); self.transition(sql)
            getattr(sql, change).add('projects' if change == 'missing' else 'unreviewed_table')
            with self.assertRaisesRegex(RuntimeError, 'unexpected_lifecycle_table_delta'):
                guard.historical_runs(sql, QUERY, 59)

    def test_target_cannot_mint_a_source_baseline_or_hide_future_tables(self):
        guard, sql = LifecycleSchemaDelta(52, 59), FixtureSql(52); self.transition(sql)
        with self.assertRaisesRegex(RuntimeError, 'source_schema_inventory_not_captured'):
            guard.historical_runs(sql, QUERY, 59)
        sql = FixtureSql(52); sql.extra.add('run_resume_requests')
        with self.assertRaisesRegex(RuntimeError, 'declared_source_has_future_migration_tables'):
            guard.historical_runs(sql, QUERY, 52)

    def test_unknown_schema_or_intermediate_generation_is_rejected(self):
        for source, target in [(51, 59), (52, 60), (59, 52), (True, 59)]:
            with self.assertRaises(RuntimeError): LifecycleSchemaDelta(source, target)
        guard, sql, _ = self.prepare()
        with self.assertRaisesRegex(RuntimeError, 'unexpected_lifecycle_schema'):
            guard.historical_runs(sql, QUERY, 58)

    def test_allowlist_exactly_matches_authoritative_migration_created_tables(self):
        migrations = Path(__file__).resolve().parents[1] / 'cowork-server/migrations'
        for version, expected in TABLES_BY_MIGRATION.items():
            paths = list(migrations.glob(f'{version:04d}_*.sql'))
            self.assertEqual(len(paths), 1)
            actual = re.findall(r'CREATE TABLE\s+([a-z_]+)', paths[0].read_text(), flags=re.IGNORECASE)
            self.assertEqual(set(actual), set(expected), 'migration table allowlist drift')


if __name__ == '__main__': unittest.main()
