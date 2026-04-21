-- ソース単位でライブラリ種別（映画 / ドラマ / 自動判定）を保持する
ALTER TABLE sources
  ADD COLUMN media_kind TEXT NOT NULL DEFAULT 'unknown'
    CHECK (media_kind IN ('movie', 'tv', 'unknown'));
