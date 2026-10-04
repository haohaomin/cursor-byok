ALTER TABLE model_configs
ADD COLUMN strip_images INTEGER NOT NULL DEFAULT 0 CHECK(strip_images IN (0, 1));
