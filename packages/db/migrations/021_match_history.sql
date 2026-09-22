-- 021: 照合履歴と監査基盤（PR2）
-- 追加のみ。起動のたびに再実行されても壊れないこと（CREATE ... IF NOT EXISTS / lenient ALTER）。
-- Jev（TypeSafe）関連のテーブルは PR3 の 022 で追加する。

-- 照合1回分。Jev の結果は PR3 で子テーブル metadata_match_jev_calls に置く
CREATE TABLE IF NOT EXISTS metadata_match_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  batch_id TEXT,
  trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('batch','single','legacy_rescan','replay')),
  mode TEXT NOT NULL CHECK (mode IN ('safe','shadow','gated')),
  evidence_class TEXT NOT NULL CHECK (evidence_class IN ('live','reconstructed_clean','historical_audit_only')),
  state_schema_version TEXT NOT NULL,
  input_snapshot_json TEXT NOT NULL,
  search_queries_json TEXT NOT NULL,
  policy_version TEXT NOT NULL,
  policy_snapshot_json TEXT NOT NULL,
  governing_matcher TEXT NOT NULL CHECK (governing_matcher IN ('rules-safe','policy')),
  decision TEXT NOT NULL CHECK (decision IN ('AUTO','REVIEW','UNRESOLVED','SKIPPED','ERROR')),
  decision_reasons_json TEXT NOT NULL,
  applied INTEGER NOT NULL DEFAULT 0 CHECK (applied IN (0,1)),
  applied_tmdb_id INTEGER,
  applied_media_type TEXT,
  status_after TEXT CHECK (status_after IN ('unmatched','pending','matched','locked')),
  tmdb_calls INTEGER NOT NULL DEFAULT 0,
  latency_ms INTEGER,
  error_text TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  CHECK (applied = 0 OR (applied_tmdb_id IS NOT NULL AND applied_media_type IN ('movie','tv'))),
  CHECK (trigger_kind <> 'replay' OR applied = 0),
  CHECK (decision <> 'ERROR' OR error_text IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_mmr_work ON metadata_match_runs(work_id, created_at);
CREATE INDEX IF NOT EXISTS idx_mmr_eval ON metadata_match_runs(evidence_class, policy_version);

-- run が見た TMDB 候補（検索結果の生値とルールのスコア）
CREATE TABLE IF NOT EXISTS metadata_match_candidates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id INTEGER NOT NULL REFERENCES metadata_match_runs(id) ON DELETE CASCADE,
  cand_key TEXT NOT NULL,
  tmdb_id INTEGER NOT NULL,
  media_type TEXT NOT NULL CHECK (media_type IN ('movie','tv')),
  search_rank INTEGER,
  rules_rank INTEGER,
  rules_score INTEGER NOT NULL,
  rules_reasons_json TEXT NOT NULL,
  tmdb_snapshot_json TEXT NOT NULL,
  explicit_conflicts_json TEXT,
  UNIQUE (run_id, cand_key),
  UNIQUE (run_id, tmdb_id, media_type)
);

-- 判定器ごとの結論（PR2 は rules-1 と rules-safe の2行）
CREATE TABLE IF NOT EXISTS metadata_match_verdicts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id INTEGER NOT NULL REFERENCES metadata_match_runs(id) ON DELETE CASCADE,
  matcher TEXT NOT NULL CHECK (matcher IN ('rules-1','rules-safe','jev-policy','jev-choice-shuffled','jev-isolated')),
  matcher_version TEXT NOT NULL,
  tmdb_id INTEGER,
  media_type TEXT,
  decision TEXT NOT NULL CHECK (decision IN ('AUTO','REVIEW','UNRESOLVED')),
  score REAL,
  reasons_json TEXT,
  CHECK ((tmdb_id IS NULL AND media_type IS NULL) OR (tmdb_id IS NOT NULL AND media_type IN ('movie','tv'))),
  UNIQUE (run_id, matcher)
);

-- レビュー対象になった理由（確定方法とは別の概念）
CREATE TABLE IF NOT EXISTS metadata_review_tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  run_id INTEGER REFERENCES metadata_match_runs(id) ON DELETE SET NULL,
  reason TEXT NOT NULL CHECK (reason IN (
    'review_decision',
    'legacy_pending',
    'audit_sample',
    'matcher_disagreement',
    'user_initiated'
  )),
  sampling_json TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  resolved_at TEXT,
  resolution_label_id INTEGER,
  CHECK (reason <> 'audit_sample' OR sampling_json IS NOT NULL)
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_mrt_open ON metadata_review_tasks(work_id, reason) WHERE resolved_at IS NULL;

-- 人間が確定した正解（確定方法 method と強さ strength。理由は review_task 側）
CREATE TABLE IF NOT EXISTS metadata_match_labels (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  run_id INTEGER REFERENCES metadata_match_runs(id) ON DELETE SET NULL,
  review_task_id INTEGER REFERENCES metadata_review_tasks(id) ON DELETE SET NULL,
  label TEXT NOT NULL CHECK (label IN ('tmdb','none')),
  tmdb_id INTEGER,
  media_type TEXT,
  method TEXT NOT NULL CHECK (method IN (
    'manual_apply',
    'manual_direct_id',
    'lock',
    'review_confirm',
    'review_pick_other',
    'review_none',
    'bulk_accept'
  )),
  strength TEXT NOT NULL CHECK (strength IN ('strong','weak')),
  reviewed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  superseded_at TEXT,
  note TEXT,
  CHECK ((label = 'tmdb' AND tmdb_id IS NOT NULL AND media_type IN ('movie','tv'))
      OR (label = 'none' AND tmdb_id IS NULL AND media_type IS NULL)),
  CHECK ((method IN ('lock','bulk_accept') AND strength = 'weak')
      OR (method NOT IN ('lock','bulk_accept') AND strength = 'strong')),
  CHECK (method <> 'review_none' OR label = 'none')
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_mml_active ON metadata_match_labels(work_id) WHERE superseded_at IS NULL;

-- 「この TMDB ID ではない」という否定の記録（照合解除・付け替え）
CREATE TABLE IF NOT EXISTS metadata_match_rejections (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  tmdb_id INTEGER NOT NULL,
  media_type TEXT NOT NULL CHECK (media_type IN ('movie','tv')),
  previous_match_source TEXT,
  source TEXT NOT NULL CHECK (source IN ('clear','repick','review_pick_other','review_none')),
  run_id INTEGER REFERENCES metadata_match_runs(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_mmrej_work ON metadata_match_rejections(work_id);

ALTER TABLE works ADD COLUMN match_source TEXT;
ALTER TABLE works ADD COLUMN last_match_run_id INTEGER;
ALTER TABLE files ADD COLUMN renamed_by_app TEXT;
