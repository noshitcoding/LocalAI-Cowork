"""Exact, read-only migration allowances for isolated lifecycle fixtures.

The baseline table inventory is captured on the declared source schema. Only
known migration-created tables may appear, and each must actually be empty.
Only historical nullable/default columns may be removed from a row comparison;
existing data, extra fields and extra rows remain part of that comparison.
"""
import json


TABLES_BY_MIGRATION = {
    53: ('management_executor_hosts', 'management_executor_host_credentials',
         'management_executor_host_releases'),
    54: ('run_waiting_reasons',),
    55: ('management_host_allocations', 'management_host_capacity_proofs',
         'management_host_capacity_challenges'),
    56: ('api_cluster_binding', 'api_cluster_nodes', 'api_cluster_executor_owners',
         'api_cluster_desktop_owners', 'api_cluster_desktop_pending'),
    57: ('run_runner_attempts',),
    58: ('api_cluster_desktop_controls',),
    59: ('run_server_execution_attempts', 'run_resume_requests'),
}
RUN_COLUMNS_BY_MIGRATION = {
    57: {'runner_cancel_id': None, 'runner_cancel_lease': None, 'runner_stop_receipt': None},
    59: {'server_resume_checkpoint_id': None},
}
CHECKPOINT_COLUMNS_BY_MIGRATION = {
    59: {'origin': 'untrusted', 'worker_id': None, 'worker_lease_token': None,
         'server_execution_attempt_id': None},
}
TABLE_INVENTORY_SQL = ("SELECT coalesce(jsonb_agg(table_name ORDER BY table_name),'[]'::jsonb) "
    "FROM information_schema.tables WHERE table_schema=current_schema() AND table_type='BASE TABLE';")


def require(condition, code):
    if not condition:
        raise RuntimeError(code)


class LifecycleSchemaDelta:
    def __init__(self, source, target):
        require(type(source) is int and type(target) is int
                and 52 <= source <= target <= 59, 'unsupported_lifecycle_schema_transition')
        self.source, self.target = source, target
        self.source_tables = None
        self.observations = []

    def migrations(self, current):
        require(type(current) is int and current in {self.source, self.target},
                'unexpected_lifecycle_schema')
        return [version for version in TABLES_BY_MIGRATION if self.source < version <= current]

    def normalize_rows(self, table, rows, current):
        versions = self.migrations(current)
        require(isinstance(rows, list) and all(isinstance(row, dict) for row in rows),
                'invalid_lifecycle_historical_rows')
        definitions = {'runs': RUN_COLUMNS_BY_MIGRATION,
                       'run_checkpoints': CHECKPOINT_COLUMNS_BY_MIGRATION}.get(table, {})
        columns = {name: default for version in versions
                   for name, default in definitions.get(version, {}).items()}
        result = []
        for row in rows:
            require(all(name in row and row[name] == default for name, default in columns.items()),
                    'historical_' + table + '_migration_state_changed')
            result.append({name: value for name, value in row.items() if name not in columns})
        return result

    def verify_inventory(self, sql, current):
        versions = self.migrations(current)
        rows = json.loads(sql(TABLE_INVENTORY_SQL))
        require(isinstance(rows, list) and all(isinstance(name, str) for name in rows)
                and len(rows) == len(set(rows)), 'invalid_lifecycle_table_inventory')
        actual = set(rows)
        if current == self.source and self.source_tables is None:
            future = {table for version, tables in TABLES_BY_MIGRATION.items()
                      if version > self.source for table in tables}
            require(not actual & future, 'declared_source_has_future_migration_tables')
            self.source_tables = actual
        require(self.source_tables is not None, 'source_schema_inventory_not_captured')
        added = {table for version in versions for table in TABLES_BY_MIGRATION[version]}
        require(actual == self.source_tables | added, 'unexpected_lifecycle_table_delta')
        counts = {}
        if added:
            # Every name is an immutable identifier from the migration allowlist.
            statement = 'SELECT jsonb_build_object(' + ','.join(
                "'" + table + "',(SELECT count(*) FROM " + table + ')'
                for table in sorted(added)) + ');'
            counts = json.loads(sql(statement))
            require(isinstance(counts, dict) and set(counts) == added
                    and all(type(value) is int and value == 0 for value in counts.values()),
                    'new_lifecycle_migration_table_not_empty')
        if 59 in versions:
            invalid = sql("SELECT count(*) FROM run_checkpoints WHERE origin <> 'untrusted' "
                "OR worker_id IS NOT NULL OR worker_lease_token IS NOT NULL "
                "OR server_execution_attempt_id IS NOT NULL;")
            require(invalid == '0', 'historical_checkpoint_provenance_changed')
        observation = {'schema': current, 'allowed_migrations': versions,
                       'verified_empty_tables': sorted(counts)}
        if observation not in self.observations:
            self.observations.append(observation)

    def historical_runs(self, sql, statement, current):
        self.verify_inventory(sql, current)
        rows = json.loads(sql(statement))
        return json.dumps(self.normalize_rows('runs', rows, current))

    def evidence(self):
        return {'source_schema': self.source, 'target_schema': self.target,
                'maximum_known_schema': 59, 'observations': self.observations,
                'helper_version': 1}
