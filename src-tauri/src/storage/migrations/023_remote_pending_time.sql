ALTER TABLE remote_devices RENAME COLUMN pending_until TO pending_ms;
UPDATE remote_devices SET pending_ms = 600000 WHERE pending_ms IS NOT NULL;
