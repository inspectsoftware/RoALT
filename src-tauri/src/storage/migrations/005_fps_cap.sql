-- Target FPS for this account's launches; NULL: the default from settings.
ALTER TABLE accounts ADD COLUMN fps_cap INTEGER;
