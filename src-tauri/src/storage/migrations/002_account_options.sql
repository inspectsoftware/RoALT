ALTER TABLE accounts ADD COLUMN allow_disconnect INTEGER NOT NULL DEFAULT 0;
-- Extra client arguments; NULL: the default from settings.
ALTER TABLE accounts ADD COLUMN launch_args TEXT;
