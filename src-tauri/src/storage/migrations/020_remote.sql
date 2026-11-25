-- Paired browser devices for the opt-in E2EE Remote Dashboard (`remote`).
CREATE TABLE remote_devices (
    id           TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    pubkey       BLOB NOT NULL,
    room_token   TEXT NOT NULL,
    role         TEXT NOT NULL DEFAULT 'control',
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER
);
