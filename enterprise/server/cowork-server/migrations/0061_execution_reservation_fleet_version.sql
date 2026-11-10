-- Existing reservations were authenticated with the original v1 Fleet key.
-- Preserve their exact completion authority when an instance later enrolls.
ALTER TABLE management_execution_reservations
    ADD COLUMN fleet_key_version SMALLINT NOT NULL DEFAULT 1
    CHECK (fleet_key_version IN (1, 2));

CREATE FUNCTION preserve_reservation_fleet_version() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.fleet_key_version IS DISTINCT FROM OLD.fleet_key_version THEN
        RAISE EXCEPTION 'execution reservation Fleet key version is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_reservation_fleet_version BEFORE UPDATE ON management_execution_reservations
    FOR EACH ROW EXECUTE FUNCTION preserve_reservation_fleet_version();
