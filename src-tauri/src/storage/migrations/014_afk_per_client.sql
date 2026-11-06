-- AltDock: anti-AFK per account (`instances::afk::client_config`). No row: the global
-- `Settings.anti_afk` (on or off, its interval, F13). `action` is the `AfkAction` JSON.
CREATE TABLE account_afk (
  account_id   INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  enabled      INTEGER NOT NULL,
  interval_sec INTEGER NOT NULL,
  action       TEXT    NOT NULL
);
