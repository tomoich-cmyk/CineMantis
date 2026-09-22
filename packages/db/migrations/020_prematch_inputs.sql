-- Migration 020: 照合前入力（pre-match input）の保存
--
-- files.file_name / file_path は NAS 整理で TMDB 由来の「タイトル (年)」に書き換わるため、
-- scan 時点の元ファイル名と source root からの相対パスを別列に残す。
--   original_captured = 1 : scan 時に取得した値
--   original_captured = 0 : 既存行を現在値から後埋めした値（NAS 整理済みの可能性あり）
-- 後埋めは db.rs で original_file_name IS NULL の行だけに行い、
-- 値が入った行の上書きは db.rs のトリガー trg_files_original_immutable で禁止する。
-- 既存 DB への再実行に備え、ALTER は apply_lenient_migration（duplicate column を無視）で流す。

ALTER TABLE files ADD COLUMN original_file_name TEXT;
ALTER TABLE files ADD COLUMN original_rel_path  TEXT;
ALTER TABLE files ADD COLUMN original_captured  INTEGER NOT NULL DEFAULT 0;
