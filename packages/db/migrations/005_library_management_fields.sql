-- Migration: library management fields for MusicBee-style browsing.
-- This migration is intentionally ALTER TABLE based so existing databases can boot.

ALTER TABLE works ADD COLUMN date_added TEXT;
ALTER TABLE works ADD COLUMN country_type TEXT DEFAULT 'unknown'
  CHECK (country_type IN ('foreign', 'domestic', 'unknown'));
ALTER TABLE works ADD COLUMN reading TEXT;
ALTER TABLE works ADD COLUMN media_category TEXT DEFAULT 'other'
  CHECK (media_category IN ('movie', 'drama', 'ova', 'other'));
ALTER TABLE works ADD COLUMN release_year INTEGER;
ALTER TABLE works ADD COLUMN genre_text TEXT;

UPDATE works
SET date_added = COALESCE(date_added, created_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    release_year = COALESCE(release_year, year),
    media_category = COALESCE(NULLIF(media_category, ''), CASE
      WHEN work_type IN ('movie', 'drama', 'ova') THEN work_type
      ELSE 'other'
    END),
    genre_text = COALESCE(genre_text, genres_json)
WHERE date_added IS NULL
   OR release_year IS NULL
   OR media_category IS NULL
   OR genre_text IS NULL;

ALTER TABLE user_stats ADD COLUMN last_watched_at TEXT;
ALTER TABLE user_stats ADD COLUMN watched_status TEXT DEFAULT 'unwatched'
  CHECK (watched_status IN ('unwatched', 'watching', 'watched', 'abandoned'));
ALTER TABLE user_stats ADD COLUMN my_rating INTEGER CHECK (my_rating BETWEEN 0 AND 10);

UPDATE user_stats
SET last_watched_at = COALESCE(last_watched_at, last_played_at),
    watched_status = COALESCE(watched_status, CASE
      WHEN watch_status = 'skipped' THEN 'abandoned'
      ELSE watch_status
    END),
    my_rating = COALESCE(my_rating, user_rating)
WHERE last_watched_at IS NULL
   OR watched_status IS NULL
   OR my_rating IS NULL;

CREATE INDEX IF NOT EXISTS idx_works_release_year ON works(release_year);
CREATE INDEX IF NOT EXISTS idx_works_media_category ON works(media_category);
CREATE INDEX IF NOT EXISTS idx_works_country_type ON works(country_type);
CREATE INDEX IF NOT EXISTS idx_works_reading ON works(reading);
CREATE INDEX IF NOT EXISTS idx_user_stats_watched_status ON user_stats(watched_status);
CREATE INDEX IF NOT EXISTS idx_user_stats_last_watched_at ON user_stats(last_watched_at);
CREATE INDEX IF NOT EXISTS idx_user_stats_my_rating ON user_stats(my_rating);
