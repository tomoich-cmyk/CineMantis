-- 022: 動画ファイルの埋め込みメタデータ（PR2.5）
-- 追加のみ。CHECK 制約の変更が必要なテーブルの作り直しは db.rs 側で
-- 条件付き（未導入のときだけ）に行う。ALTER は apply_lenient_migration で流す。

-- ffprobe の format.tags（allowlist）と各ストリームの言語
ALTER TABLE files ADD COLUMN container_tags_json TEXT;
-- 1 = scan 時に取得、0 = 後から後埋め
ALTER TABLE files ADD COLUMN tags_captured INTEGER NOT NULL DEFAULT 0;
ALTER TABLE files ADD COLUMN tags_captured_at TEXT;
-- format.tags.encoder（作成経路の手がかり）
ALTER TABLE files ADD COLUMN tags_encoder TEXT;
-- provider_known / pipeline_known / unknown / suspicious
ALTER TABLE files ADD COLUMN tags_provenance TEXT;
-- comment の URL などから分かった配信元
ALTER TABLE files ADD COLUMN tags_provider_hint TEXT;
-- 長さ上限で値を切り詰めたか
ALTER TABLE files ADD COLUMN tags_truncated INTEGER NOT NULL DEFAULT 0;

-- 候補がどの検索語から出てきたか（legacy / embedded / both）。
-- rules-1 を旧経路のまま評価するために必要
ALTER TABLE metadata_match_candidates ADD COLUMN query_source TEXT;
