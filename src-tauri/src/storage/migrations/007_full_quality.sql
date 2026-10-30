-- This account's clients keep the user's own Roblox graphics (no lowest graphics or lighter
-- rendering): the main account.
ALTER TABLE accounts ADD COLUMN full_quality INTEGER NOT NULL DEFAULT 0;
