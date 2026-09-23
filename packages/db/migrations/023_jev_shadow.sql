-- 023: Jev shadow 層の記録（PR3 C1）
--
-- 追加のみ。CHECK 制約の変更が必要なテーブルは無いので、022 のような作り直しは行わない。
--   - Jev の最終判断は metadata_match_verdicts の matcher='jev-policy'（既存 CHECK のまま）
--   - packed / shuffled / isolated の生の呼び出しは metadata_match_jev_calls に持つ
-- 起動のたびに再実行されても壊れないこと（CREATE ... IF NOT EXISTS / ALTER は lenient 経由）。

-- 複合外部キーの親になるための UNIQUE インデックス。
-- 単独 FK だけだと「run A の call が run B の候補を指す」ことを防げないため、
-- (run_id, id) の組で参照させる。SQLite は参照先に UNIQUE が無いと
-- 実行時に "foreign key mismatch" になるので、子テーブルより先に作る。
CREATE UNIQUE INDEX IF NOT EXISTS ux_mmc_run_candidate
  ON metadata_match_candidates(run_id, id);
-- ID と cand_key が同じ候補を指すことまで保証するための親キー。
-- (run_id, id) が一意なので cand_key を足しても一意のまま。
CREATE UNIQUE INDEX IF NOT EXISTS ux_mmc_run_candidate_key
  ON metadata_match_candidates(run_id, id, cand_key);

-- 質問テンプレートと検証規則の凍結。追記のみで、文面を変えたら contract_version を上げる。
-- canonical_sha256 は state_schema_version + instructions + questions_template + criteria +
-- validation_policy を canonical JSON 化して作る（整形の違いで hash が変わらないように）。
-- モデル version は hash に含めない（別列・call 側の requested_model が正）。
CREATE TABLE IF NOT EXISTS jev_contracts (
  contract_version        TEXT PRIMARY KEY,
  state_schema_version    TEXT NOT NULL,
  instructions_text       TEXT NOT NULL,
  questions_template_json TEXT NOT NULL,
  criteria_json           TEXT NOT NULL,
  validation_policy_json  TEXT NOT NULL,
  canonical_sha256        TEXT NOT NULL,
  default_model           TEXT NOT NULL,
  created_at              TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- 1 run に対する Jev の呼び出し。isolated は候補ごとに複数入るので call_seq で一意にする。
-- API キー・HTTP ヘッダは絶対に保存しない。
CREATE TABLE IF NOT EXISTS metadata_match_jev_calls (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id INTEGER NOT NULL REFERENCES metadata_match_runs(id) ON DELETE CASCADE,
  call_seq INTEGER NOT NULL,
  call_kind TEXT NOT NULL CHECK (call_kind IN ('packed','shuffled','isolated')),
  subject_candidate_id INTEGER,
  subject_cand_key TEXT,

  contract_version TEXT NOT NULL REFERENCES jev_contracts(contract_version),
  state_schema_version TEXT NOT NULL,
  requested_model TEXT NOT NULL,
  response_model TEXT,
  enrichment_profile TEXT CHECK (enrichment_profile IN ('movie-v1','tv-v1','none')),

  state_json TEXT NOT NULL,
  questions_json TEXT NOT NULL,
  candidate_order_json TEXT NOT NULL,
  state_bytes INTEGER,

  status TEXT NOT NULL CHECK (status IN ('ok','invalid','unavailable','error','skipped')),
  http_status INTEGER,
  response_json TEXT,
  response_sha256 TEXT,
  response_prefix TEXT,
  response_truncated INTEGER NOT NULL DEFAULT 0 CHECK (response_truncated IN (0,1)),
  parsed_answer_json TEXT,

  selected_candidate_id INTEGER,
  selected_cand_key TEXT,
  answered_none INTEGER NOT NULL DEFAULT 0 CHECK (answered_none IN (0,1)),

  input_tokens INTEGER,
  output_tokens INTEGER,
  latency_ms INTEGER,
  retry_count INTEGER NOT NULL DEFAULT 0,
  error_kind TEXT CHECK (error_kind IN
    ('auth','rate_limit','unprocessable','overloaded','network','timeout',
     'parse','response_too_large','model_mismatch','other')),
  error_text TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),

  UNIQUE (run_id, call_seq),
  -- 複合 UNIQUE。metadata_match_candidate_scores から (run_id, id) で参照する
  UNIQUE (run_id, id),
  -- 参照する候補は必ず同じ run のもので、ID と cand_key も一致していること。
  -- ID と key を両方保存する以上、食い違いを DB 側で拒否する。
  FOREIGN KEY (run_id, subject_candidate_id, subject_cand_key)
    REFERENCES metadata_match_candidates(run_id, id, cand_key) ON DELETE CASCADE,
  FOREIGN KEY (run_id, selected_candidate_id, selected_cand_key)
    REFERENCES metadata_match_candidates(run_id, id, cand_key) ON DELETE CASCADE,
  -- 片方だけ NULL にすると複合外部キーの検査が素通りするので、両方 NULL か両方あるか
  CHECK ((subject_candidate_id IS NULL AND subject_cand_key IS NULL)
      OR (subject_candidate_id IS NOT NULL AND subject_cand_key IS NOT NULL)),
  CHECK ((selected_candidate_id IS NULL AND selected_cand_key IS NULL)
      OR (selected_candidate_id IS NOT NULL AND selected_cand_key IS NOT NULL)),
  CHECK (call_kind <> 'isolated' OR subject_candidate_id IS NOT NULL),
  CHECK (status <> 'ok' OR parsed_answer_json IS NOT NULL),
  CHECK (answered_none = 0 OR selected_candidate_id IS NULL),
  -- 設計どおり hash と先頭 prefix を必ず残す
  CHECK (response_truncated = 0
      OR (response_sha256 IS NOT NULL AND response_prefix IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS idx_jev_calls_run ON metadata_match_jev_calls(run_id, call_kind);
CREATE INDEX IF NOT EXISTS idx_jev_calls_status ON metadata_match_jev_calls(status, created_at);

-- 候補ごとの matcher 別 score / rank。
-- metadata_match_candidates.rules_score / rules_rank は combined candidate set 上の値なので、
-- matcher 固有の値はこちらに持つ。in_candidate_set で「入力に入っていない候補」と
-- 「入力に入っていたが選ばれなかった候補」を区別する。
CREATE TABLE IF NOT EXISTS metadata_match_candidate_scores (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id INTEGER NOT NULL REFERENCES metadata_match_runs(id) ON DELETE CASCADE,
  -- candidate score は必ず既存候補に属する
  candidate_id INTEGER NOT NULL,
  cand_key TEXT NOT NULL,
  -- score の出どころ（経路）。判定器そのものではない。
  --   legacy            … 旧経路の検索・採点（matcher_version = 'rules-1' など）
  --   rules-tags-shadow … 影判定の採点（matcher_version = 'rules-tags-shadow-2' など）
  --   jev-call          … Jev の候補評価（contract / model は jev_call_id から辿る）
  -- guard 込みの判定器である rules-safe は、候補スコアの出どころとしては使わない。
  matcher TEXT NOT NULL CHECK (matcher IN ('legacy','rules-tags-shadow','jev-call')),
  -- 決定的な matcher でも、後から採点式の版を識別できるように必ず入れる
  matcher_version TEXT NOT NULL,
  score REAL,
  rank INTEGER,
  in_candidate_set INTEGER NOT NULL DEFAULT 1 CHECK (in_candidate_set IN (0,1)),
  reasons_json TEXT,
  jev_call_id INTEGER,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  -- 候補も call も、必ず同じ run のもの。候補は ID と cand_key の一致まで保証する
  FOREIGN KEY (run_id, candidate_id, cand_key)
    REFERENCES metadata_match_candidates(run_id, id, cand_key) ON DELETE CASCADE,
  FOREIGN KEY (run_id, jev_call_id)
    REFERENCES metadata_match_jev_calls(run_id, id) ON DELETE CASCADE,
  -- jev_call_id の意味を双方向で固定する。
  -- 決定的な matcher に call を付けられると、部分 UNIQUE インデックス
  -- （jev_call_id IS NULL 側）を迂回して同じ matcher の行を複数作れてしまう。
  CHECK ((matcher =  'jev-call' AND jev_call_id IS NOT NULL)
      OR (matcher <> 'jev-call' AND jev_call_id IS NULL))
);
CREATE INDEX IF NOT EXISTS idx_cand_scores_run ON metadata_match_candidate_scores(run_id, matcher);
-- 決定的な matcher は run × 候補 × matcher で1行。
-- SQLite は UNIQUE 内の NULL を互いに異なる値として扱うため、部分インデックスで分ける。
CREATE UNIQUE INDEX IF NOT EXISTS ux_cand_scores_deterministic
  ON metadata_match_candidate_scores(run_id, cand_key, matcher, matcher_version)
  WHERE jev_call_id IS NULL;
-- Jev は call ごとに候補の評価が付く
CREATE UNIQUE INDEX IF NOT EXISTS ux_cand_scores_jev
  ON metadata_match_candidate_scores(run_id, cand_key, matcher, matcher_version, jev_call_id)
  WHERE jev_call_id IS NOT NULL;
