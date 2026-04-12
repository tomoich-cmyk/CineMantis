-- CineMantis SQLite Schema v0.1
-- Migration 001: Initial tables

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- ─── sources ────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS sources (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  name          TEXT    NOT NULL,
  root_path     TEXT    NOT NULL UNIQUE,
  source_type   TEXT    NOT NULL CHECK (source_type IN ('nas','external_hdd','local')),
  is_enabled    INTEGER NOT NULL DEFAULT 1,
  status        TEXT    NOT NULL DEFAULT 'online' CHECK (status IN ('online','offline','error')),
  last_scan_at  TEXT,
  last_seen_at  TEXT
);

-- ─── files ──────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS files (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id           INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  file_path           TEXT    NOT NULL,
  file_name           TEXT    NOT NULL,
  extension           TEXT,
  file_size           INTEGER,
  mtime               TEXT,
  ctime               TEXT,
  duration_sec        REAL,
  width               INTEGER,
  height              INTEGER,
  video_codec         TEXT,
  audio_codec         TEXT,
  container           TEXT,
  hash_partial        TEXT,
  availability_status TEXT    NOT NULL DEFAULT 'available'
                              CHECK (availability_status IN ('available','missing','offline')),
  created_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (source_id, file_path)
);

CREATE INDEX IF NOT EXISTS idx_files_source ON files(source_id);
CREATE INDEX IF NOT EXISTS idx_files_availability ON files(availability_status);

-- ─── works ──────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS works (
  id                      INTEGER PRIMARY KEY AUTOINCREMENT,
  work_type               TEXT    NOT NULL CHECK (work_type IN ('movie','drama','ova','special','other')),
  title                   TEXT    NOT NULL,
  original_title          TEXT,
  sort_title              TEXT,
  year                    INTEGER,
  country                 TEXT,
  synopsis                TEXT,
  runtime_sec             REAL,
  genres_json             TEXT,   -- JSON array e.g. '["SF","Thriller"]'
  poster_path             TEXT,
  thumb_path              TEXT,
  external_rating         REAL,
  external_rating_source  TEXT,
  tmdb_id                 INTEGER,
  imdb_id                 TEXT,
  match_status            TEXT    NOT NULL DEFAULT 'unmatched'
                                  CHECK (match_status IN ('unmatched','pending','matched','locked')),
  match_confidence        REAL,
  release_date            TEXT,
  created_at              TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at              TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_works_type  ON works(work_type);
CREATE INDEX IF NOT EXISTS idx_works_year  ON works(year);
CREATE INDEX IF NOT EXISTS idx_works_tmdb  ON works(tmdb_id);
CREATE INDEX IF NOT EXISTS idx_works_title ON works(sort_title, title);

-- ─── work_parts ─────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS work_parts (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id     INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  part_no     INTEGER NOT NULL DEFAULT 1,
  part_label  TEXT,
  start_sec   REAL,
  end_sec     REAL,
  play_order  INTEGER NOT NULL DEFAULT 1,
  UNIQUE (work_id, part_no)
);

CREATE INDEX IF NOT EXISTS idx_work_parts_work ON work_parts(work_id);
CREATE INDEX IF NOT EXISTS idx_work_parts_file ON work_parts(file_id);

-- ─── user_stats ─────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS user_stats (
  work_id             INTEGER PRIMARY KEY REFERENCES works(id) ON DELETE CASCADE,
  user_rating         INTEGER CHECK (user_rating BETWEEN 1 AND 5),
  play_count          INTEGER NOT NULL DEFAULT 0,
  last_played_at      TEXT,
  resume_position_sec REAL,
  is_favorite         INTEGER NOT NULL DEFAULT 0,
  watch_status        TEXT    NOT NULL DEFAULT 'unwatched'
                              CHECK (watch_status IN ('unwatched','watching','watched','skipped')),
  personal_note       TEXT,
  updated_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_user_stats_watch  ON user_stats(watch_status);
CREATE INDEX IF NOT EXISTS idx_user_stats_rating ON user_stats(user_rating);
CREATE INDEX IF NOT EXISTS idx_user_stats_fav    ON user_stats(is_favorite);

-- ─── tags ───────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS tags (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  name     TEXT    NOT NULL UNIQUE,
  color    TEXT,
  tag_type TEXT    NOT NULL DEFAULT 'user' CHECK (tag_type IN ('user','smart','system'))
);

-- ─── work_tags ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS work_tags (
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  tag_id  INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
  PRIMARY KEY (work_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_work_tags_tag ON work_tags(tag_id);

-- ─── persons ────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS persons (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  name             TEXT    NOT NULL,
  original_name    TEXT,
  sort_name        TEXT,
  person_type_hint TEXT,
  tmdb_person_id   INTEGER,
  thumb_path       TEXT,
  created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_persons_tmdb ON persons(tmdb_person_id);
CREATE INDEX IF NOT EXISTS idx_persons_name ON persons(sort_name, name);

-- ─── work_persons ────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS work_persons (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id        INTEGER NOT NULL REFERENCES works(id)   ON DELETE CASCADE,
  person_id      INTEGER NOT NULL REFERENCES persons(id) ON DELETE CASCADE,
  role_type      TEXT    NOT NULL CHECK (role_type IN ('director','screenplay','cast','producer','music','other')),
  billing_order  INTEGER,
  character_name TEXT,
  UNIQUE (work_id, person_id, role_type)
);

CREATE INDEX IF NOT EXISTS idx_work_persons_work   ON work_persons(work_id);
CREATE INDEX IF NOT EXISTS idx_work_persons_person ON work_persons(person_id, role_type);

-- ─── series ─────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS series (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  name              TEXT NOT NULL,
  original_name     TEXT,
  series_type       TEXT NOT NULL CHECK (series_type IN ('movie_series','drama_series','collection')),
  tmdb_id           INTEGER,
  external_source   TEXT,
  poster_path       TEXT,
  sort_mode_default TEXT,
  created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- ─── series_items ────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS series_items (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  series_id         INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
  work_id           INTEGER NOT NULL REFERENCES works(id)  ON DELETE CASCADE,
  season_no         INTEGER,
  episode_no        INTEGER,
  series_order      REAL,
  display_group     TEXT,
  is_primary_series INTEGER NOT NULL DEFAULT 1,
  UNIQUE (series_id, work_id)
);

CREATE INDEX IF NOT EXISTS idx_series_items_series ON series_items(series_id, series_order);
CREATE INDEX IF NOT EXISTS idx_series_items_work   ON series_items(work_id);

-- ─── Triggers: auto-update updated_at ────────────────────────────────────────
CREATE TRIGGER IF NOT EXISTS trg_files_updated_at
  AFTER UPDATE ON files
  BEGIN UPDATE files SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id; END;

CREATE TRIGGER IF NOT EXISTS trg_works_updated_at
  AFTER UPDATE ON works
  BEGIN UPDATE works SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id; END;

CREATE TRIGGER IF NOT EXISTS trg_user_stats_updated_at
  AFTER UPDATE ON user_stats
  BEGIN UPDATE user_stats SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE work_id = NEW.work_id; END;

CREATE TRIGGER IF NOT EXISTS trg_persons_updated_at
  AFTER UPDATE ON persons
  BEGIN UPDATE persons SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id; END;

CREATE TRIGGER IF NOT EXISTS trg_series_updated_at
  AFTER UPDATE ON series
  BEGIN UPDATE series SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id; END;
