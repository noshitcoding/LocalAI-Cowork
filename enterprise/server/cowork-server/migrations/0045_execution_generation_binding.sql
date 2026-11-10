-- Freeze the execution generation before infrastructure work starts. A delayed
-- receipt must settle this exact transition, not whichever generation is current.
ALTER TABLE management_operations ADD COLUMN execution_binding JSONB;
