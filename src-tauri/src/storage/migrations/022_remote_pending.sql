-- A paired device stays pending until its first session; one that never connects expires.
ALTER TABLE remote_devices ADD COLUMN pending_until INTEGER;
ALTER TABLE remote_devices ADD COLUMN replaces TEXT;
