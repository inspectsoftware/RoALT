-- The servers an account's clients reached, newest kept (`Db::add_server`). No access codes: a
-- private server is only listed.
CREATE TABLE server_history (
  id         INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  place_id   INTEGER NOT NULL,
  job_id     TEXT NOT NULL,
  private    INTEGER NOT NULL DEFAULT 0,
  joined_at  INTEGER NOT NULL
);
CREATE INDEX server_history_account ON server_history(account_id, id);
