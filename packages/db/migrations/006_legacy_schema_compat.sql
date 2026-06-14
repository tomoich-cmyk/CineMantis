-- Compatibility bridge for databases created by the original schema.

ALTER TABLE series ADD COLUMN title TEXT;
ALTER TABLE series ADD COLUMN name TEXT;
ALTER TABLE series ADD COLUMN sort_title TEXT;
ALTER TABLE series ADD COLUMN overview TEXT;

UPDATE series
SET title = COALESCE(title, name),
    name = COALESCE(name, title),
    sort_title = COALESCE(sort_title, title, name);

ALTER TABLE series_items ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
UPDATE series_items SET sort_order = COALESCE(sort_order, CAST(series_order AS INTEGER), 0);

ALTER TABLE persons ADD COLUMN tmdb_id INTEGER;
ALTER TABLE persons ADD COLUMN profile_path TEXT;
ALTER TABLE persons ADD COLUMN tmdb_person_id INTEGER;
ALTER TABLE persons ADD COLUMN thumb_path TEXT;
UPDATE persons
SET tmdb_id = COALESCE(tmdb_id, tmdb_person_id),
    tmdb_person_id = COALESCE(tmdb_person_id, tmdb_id),
    profile_path = COALESCE(profile_path, thumb_path),
    thumb_path = COALESCE(thumb_path, profile_path);

ALTER TABLE work_persons ADD COLUMN role TEXT;
ALTER TABLE work_persons ADD COLUMN display_order INTEGER NOT NULL DEFAULT 0;
ALTER TABLE work_persons ADD COLUMN role_type TEXT;
ALTER TABLE work_persons ADD COLUMN billing_order INTEGER;
UPDATE work_persons
SET role = COALESCE(role, CASE role_type WHEN 'screenplay' THEN 'writer' ELSE role_type END),
    role_type = COALESCE(role_type, CASE role WHEN 'writer' THEN 'screenplay' ELSE role END),
    display_order = COALESCE(display_order, billing_order, 0),
    billing_order = COALESCE(billing_order, display_order, 0);

CREATE UNIQUE INDEX IF NOT EXISTS idx_persons_tmdb_id_compat ON persons(tmdb_id);
CREATE INDEX IF NOT EXISTS idx_work_persons_role_compat ON work_persons(role);
CREATE UNIQUE INDEX IF NOT EXISTS idx_series_tmdb_id_compat ON series(tmdb_id) WHERE tmdb_id IS NOT NULL;
