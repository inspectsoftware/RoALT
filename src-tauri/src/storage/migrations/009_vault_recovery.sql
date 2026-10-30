-- Its own table: saving the vault record is INSERT OR REPLACE, which would wipe extra columns.
CREATE TABLE vault_recovery (
  id           INTEGER PRIMARY KEY CHECK (id = 1),
  wrapped_key  BLOB    NOT NULL,
  kdf_salt     BLOB    NOT NULL,
  kdf_m_kib    INTEGER NOT NULL,
  kdf_t        INTEGER NOT NULL,
  kdf_p        INTEGER NOT NULL
);
