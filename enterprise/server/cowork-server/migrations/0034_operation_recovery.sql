-- An expired execution lease never authorizes replay. An explicit recovery
-- request permits only stopping resources when no durable completion exists.
ALTER TABLE management_operations
    ADD COLUMN recovery_requested_by UUID REFERENCES users(id),
    ADD COLUMN recovery_requested_at TIMESTAMPTZ,
    ADD COLUMN recovery_dispatched_at TIMESTAMPTZ;
