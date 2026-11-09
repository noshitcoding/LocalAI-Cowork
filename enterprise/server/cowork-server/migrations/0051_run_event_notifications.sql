-- Notifications contain only a run identifier and are emitted at transaction
-- commit. Durable event data and sequence cursors remain in run_events. A row
-- trigger covers all writers, including worker/executor and administrative SQL.
CREATE FUNCTION notify_cowork_run_event() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('cowork_run_events', NEW.run_id::text);
    RETURN NEW;
END;
$$;

CREATE TRIGGER cowork_run_event_notification
AFTER INSERT ON run_events
FOR EACH ROW EXECUTE FUNCTION notify_cowork_run_event();
