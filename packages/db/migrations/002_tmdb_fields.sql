-- Migration 002: TMDb連携フィールドと app_settings

-- works へのカラム追加
ALTER TABLE works ADD COLUMN title_guess        TEXT;
ALTER TABLE works ADD COLUMN media_kind         TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE works ADD COLUMN tmdb_media_type    TEXT;
ALTER TABLE works ADD COLUMN country_json       TEXT;
ALTER TABLE works ADD COLUMN metadata_updated_at TEXT;

-- アプリ設定テーブル（APIキー等を保存）
CREATE TABLE IF NOT EXISTS app_settings (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- インデックス補足
CREATE INDEX IF NOT EXISTS idx_works_media_kind    ON works(media_kind);
CREATE INDEX IF NOT EXISTS idx_works_match_status2 ON works(match_status, tmdb_id);
