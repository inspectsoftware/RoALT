CREATE TABLE vault (
  id           INTEGER PRIMARY KEY CHECK (id = 1),
  mode         TEXT    NOT NULL CHECK (mode IN ('dpapi', 'master')),
  wrapped_key  BLOB    NOT NULL,
  kdf_salt     BLOB,
  kdf_m_kib    INTEGER,
  kdf_t        INTEGER,
  kdf_p        INTEGER
);

CREATE TABLE accounts (
  id                INTEGER PRIMARY KEY,
  user_id           INTEGER NOT NULL UNIQUE,
  username          TEXT    NOT NULL,
  display_name      TEXT    NOT NULL,
  alias             TEXT    NOT NULL DEFAULT '',
  notes             TEXT    NOT NULL DEFAULT '',
  color             TEXT,
  favourite         INTEGER NOT NULL DEFAULT 0,
  sort_order        INTEGER NOT NULL,
  cookie            BLOB    NOT NULL,
  status            TEXT    NOT NULL DEFAULT 'unknown'
                    CHECK (status IN ('unknown', 'valid', 'invalid', 'challenged')),
  status_checked_at INTEGER,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);

CREATE TABLE groups (
  id         INTEGER PRIMARY KEY,
  name       TEXT    NOT NULL UNIQUE COLLATE NOCASE,
  color      TEXT,
  sort_order INTEGER NOT NULL
);

CREATE TABLE account_groups (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  group_id   INTEGER NOT NULL REFERENCES groups(id)   ON DELETE CASCADE,
  PRIMARY KEY (account_id, group_id)
) WITHOUT ROWID;

CREATE TABLE favourites (
  id           INTEGER PRIMARY KEY,
  name         TEXT    NOT NULL,
  target       BLOB    NOT NULL,
  account_ids  TEXT    NOT NULL DEFAULT '[]',
  in_tray      INTEGER NOT NULL DEFAULT 1,
  sort_order   INTEGER NOT NULL,
  last_used_at INTEGER
);

CREATE TABLE cache (
  key        TEXT    PRIMARY KEY,
  value      TEXT    NOT NULL,
  fetched_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;
