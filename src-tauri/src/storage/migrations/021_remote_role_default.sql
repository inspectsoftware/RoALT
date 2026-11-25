-- New remote devices default to View only.
CREATE TABLE remote_devices_new (
    id           TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    pubkey       BLOB NOT NULL,
    room_token   TEXT NOT NULL,
    role         TEXT NOT NULL DEFAULT 'readonly',
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER
);
INSERT INTO remote_devices_new (id, label, pubkey, room_token, role, created_at, last_seen_at)
    SELECT id, label, pubkey, room_token, role, created_at, last_seen_at FROM remote_devices;
DROP TABLE remote_devices;
ALTER TABLE remote_devices_new RENAME TO remote_devices;
