ALTER TABLE interactions ADD COLUMN IF NOT EXISTS tag TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS ix_interactions_tag ON interactions (service_name, tag);
