-- Existing runs keep their legacy private workspace. New bindings are assigned
-- only by the opt-in server path, never by a client-supplied sandbox description.
CREATE TABLE chat_workspaces (
    thread_id UUID PRIMARY KEY REFERENCES threads(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    workspace_id UUID NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (thread_id, project_id, workspace_id)
);
ALTER TABLE runs ADD COLUMN sandbox_workspace_id UUID;
ALTER TABLE runs ADD CONSTRAINT runs_chat_workspace_fk
    FOREIGN KEY (thread_id, project_id, sandbox_workspace_id)
    REFERENCES chat_workspaces(thread_id, project_id, workspace_id);

CREATE FUNCTION protect_chat_workspace() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'chat_workspaces' THEN
        RAISE EXCEPTION 'chat workspace identity is immutable';
    END IF;
    IF NEW.sandbox_workspace_id IS DISTINCT FROM OLD.sandbox_workspace_id
       OR (OLD.sandbox_workspace_id IS NOT NULL AND
           (NEW.thread_id IS DISTINCT FROM OLD.thread_id OR
            NEW.project_id IS DISTINCT FROM OLD.project_id OR
            NEW.target_kind IS DISTINCT FROM OLD.target_kind)) THEN
        RAISE EXCEPTION 'run workspace binding is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER chat_workspace_identity BEFORE UPDATE OR DELETE ON chat_workspaces
    FOR EACH ROW EXECUTE FUNCTION protect_chat_workspace();
CREATE TRIGGER run_workspace_binding BEFORE UPDATE ON runs
    FOR EACH ROW EXECUTE FUNCTION protect_chat_workspace();

-- An older API/worker must fail instead of silently using a per-run volume after
-- a chat has opted into persistence. This is a deployment compatibility fence,
-- not a substitute for API authorization or database credential isolation.
CREATE FUNCTION require_chat_workspace_contract() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE bound UUID;
BEGIN
    IF TG_TABLE_NAME = 'runs' THEN
        IF TG_OP = 'INSERT' THEN
            IF NEW.target_kind = 'server_linux' THEN
                SELECT workspace_id INTO bound FROM chat_workspaces WHERE thread_id=NEW.thread_id;
                IF bound IS NOT NULL AND NEW.sandbox_workspace_id IS DISTINCT FROM bound THEN
                    RAISE EXCEPTION 'chat workspace binding is required';
                END IF;
            END IF;
        ELSIF NEW.lease_token IS NULL OR NEW.lease_token IS NOT DISTINCT FROM OLD.lease_token THEN
            RETURN NEW;
        END IF;
        bound := NEW.sandbox_workspace_id;
    ELSE
        IF TG_TABLE_NAME = 'terminal_sessions' AND TG_OP = 'UPDATE' THEN
            IF NEW.state <> 'connected' OR NEW.state IS NOT DISTINCT FROM OLD.state THEN
                RETURN NEW;
            END IF;
        END IF;
        SELECT sandbox_workspace_id INTO bound FROM runs WHERE id=NEW.run_id;
    END IF;
    IF bound IS NOT NULL AND current_setting('cowork.chat_workspace_version',true) IS DISTINCT FROM '1' THEN
        RAISE EXCEPTION 'chat workspace contract version 1 is required';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER run_workspace_contract BEFORE INSERT OR UPDATE ON runs
    FOR EACH ROW EXECUTE FUNCTION require_chat_workspace_contract();
CREATE TRIGGER desktop_workspace_contract BEFORE INSERT ON desktop_sessions
    FOR EACH ROW EXECUTE FUNCTION require_chat_workspace_contract();
CREATE TRIGGER terminal_workspace_contract BEFORE INSERT OR UPDATE ON terminal_sessions
    FOR EACH ROW EXECUTE FUNCTION require_chat_workspace_contract();
