-- No Discord alerts about this account's clients.
ALTER TABLE accounts ADD COLUMN discord_muted INTEGER NOT NULL DEFAULT 0;
