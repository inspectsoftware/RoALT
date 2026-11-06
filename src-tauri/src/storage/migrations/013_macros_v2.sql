-- AltDock: the user's AutoHotkey v2 macros are back (`macros`, as in 011 but without the global
-- run_on_launch: each account picks its own macro in `account_macros`). `trusted`: typed in the
-- app, pushed from the main app, or its source reviewed; imported and restored ones start at 0.
-- `hotkey` '' for none.
CREATE TABLE IF NOT EXISTS macros (
  id         INTEGER PRIMARY KEY,
  name       TEXT    NOT NULL UNIQUE COLLATE NOCASE,
  body       TEXT    NOT NULL,
  trusted    INTEGER NOT NULL DEFAULT 0,
  hotkey     TEXT    NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
-- The macro an account's clients run. `autorun`: it starts when the client reaches a game.
CREATE TABLE account_macros (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  macro_id   INTEGER NOT NULL REFERENCES macros(id) ON DELETE CASCADE,
  autorun    INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX account_macros_macro ON account_macros(macro_id);
