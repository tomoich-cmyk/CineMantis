-- Migration 003: シリーズ管理テーブル

-- シリーズ（映画コレクション / TVシリーズ / 手動グループ）
CREATE TABLE IF NOT EXISTS series (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT    NOT NULL,
    sort_title   TEXT,
    series_type  TEXT    NOT NULL DEFAULT 'manual',  -- 'movie_collection' | 'tv_show' | 'manual'
    tmdb_id      INTEGER,                             -- collection_id or tv tmdb_id
    poster_path  TEXT,                                -- ローカルポスターパス
    overview     TEXT,
    created_at   TEXT    DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at   TEXT    DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_series_tmdb_id ON series(tmdb_id, series_type);

-- シリーズ ↔ 作品 中間テーブル
CREATE TABLE IF NOT EXISTS series_items (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id  INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    work_id    INTEGER NOT NULL REFERENCES works(id)  ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0,   -- 年 or episode sort
    season_no  INTEGER,
    episode_no INTEGER,
    UNIQUE (series_id, work_id)
);

CREATE INDEX IF NOT EXISTS idx_series_items_series_id ON series_items(series_id);
CREATE INDEX IF NOT EXISTS idx_series_items_work_id   ON series_items(work_id);

-- works テーブルへのカラム追加
ALTER TABLE works ADD COLUMN tmdb_collection_id INTEGER;
ALTER TABLE works ADD COLUMN season_no  INTEGER;
ALTER TABLE works ADD COLUMN episode_no INTEGER;
