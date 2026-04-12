-- Migration 004: 人物管理テーブル

-- 人物マスタ
CREATE TABLE IF NOT EXISTS persons (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tmdb_id      INTEGER UNIQUE,
    name         TEXT    NOT NULL,
    profile_path TEXT,    -- ローカルキャッシュパス（将来用）
    created_at   TEXT    DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_persons_tmdb_id ON persons(tmdb_id);
CREATE INDEX IF NOT EXISTS idx_persons_name    ON persons(name);

-- 作品 ↔ 人物 中間テーブル
CREATE TABLE IF NOT EXISTS work_persons (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    work_id        INTEGER NOT NULL REFERENCES works(id)   ON DELETE CASCADE,
    person_id      INTEGER NOT NULL REFERENCES persons(id) ON DELETE CASCADE,
    role           TEXT    NOT NULL,   -- 'director' | 'writer' | 'cast'
    character_name TEXT,               -- 出演者のキャラクター名
    display_order  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (work_id, person_id, role)
);

CREATE INDEX IF NOT EXISTS idx_work_persons_work_id   ON work_persons(work_id);
CREATE INDEX IF NOT EXISTS idx_work_persons_person_id ON work_persons(person_id);
CREATE INDEX IF NOT EXISTS idx_work_persons_role      ON work_persons(role);
