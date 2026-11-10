-- Explicit, scoped infrastructure stop evidence; never inferred from lease age.
ALTER TABLE management_operations ADD COLUMN execution_recovery_binding JSONB;
ALTER TABLE management_execution_reservations ADD COLUMN recovery_operation_id UUID REFERENCES management_operations(id);
ALTER TABLE management_execution_reservations ADD CONSTRAINT recovered_execution_is_released
    CHECK (recovery_operation_id IS NULL OR state='released');
