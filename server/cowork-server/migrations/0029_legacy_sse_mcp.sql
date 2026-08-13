ALTER TABLE server_mcp_bindings
    DROP CONSTRAINT server_mcp_bindings_transport_check;

ALTER TABLE server_mcp_bindings
    ADD CONSTRAINT server_mcp_bindings_transport_check
    CHECK (transport IN ('stdio', 'streamable_http', 'sse'));
