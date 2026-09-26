-- 025: production の safe run と Jev shadow sibling run の対応（PR3 C4c）
--
-- Jev は shadow でしか動かさないが、C4b は mode='shadow' かつ applied=0 の run しか
-- 受け付けない。production の run は mode='safe' のままにしたいので、safe run 1 件に
-- 対して shadow の兄弟 run を 1 件作り、その対応をここに持つ。
--   - safe_run_id が PRIMARY KEY なので、1 つの safe run に shadow は 1 件だけ
--   - shadow_run_id も UNIQUE なので、1 つの shadow run が複数の safe run に紐づかない
-- 起動のたびに再実行されても壊れないこと。

CREATE TABLE IF NOT EXISTS metadata_match_jev_run_links (
    safe_run_id INTEGER PRIMARY KEY
        REFERENCES metadata_match_runs(id) ON DELETE CASCADE,

    shadow_run_id INTEGER NOT NULL UNIQUE
        REFERENCES metadata_match_runs(id) ON DELETE CASCADE,

    created_at TEXT NOT NULL DEFAULT
        (strftime('%Y-%m-%dT%H:%M:%fZ','now')),

    CHECK (safe_run_id <> shadow_run_id)
);
