-- Clients that crashed, were disconnected or hung; the newest 200 are kept.
CREATE TABLE crashes (
  id          INTEGER PRIMARY KEY,
  instance_id TEXT    NOT NULL,
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind        TEXT    NOT NULL,
  at          INTEGER NOT NULL,
  uptime_ms   INTEGER NOT NULL,
  reason      TEXT,
  place_id    INTEGER,
  log_path    TEXT
);
