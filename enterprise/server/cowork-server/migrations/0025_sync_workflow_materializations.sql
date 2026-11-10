ALTER TABLE sync_materializations
    DROP CONSTRAINT sync_materializations_entity_type_check;

ALTER TABLE sync_materializations
    ADD CONSTRAINT sync_materializations_entity_type_check CHECK (entity_type IN (
        'project', 'thread', 'message', 'task', 'schedule', 'run', 'crew', 'skill',
        'memory', 'provider_profile', 'secret_metadata', 'mcp_metadata'
    ));
