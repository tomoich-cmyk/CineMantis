-- 024: Jev shadow evaluation の session と予算（PR3 C4a）
--
-- 追加のみ。023 のテーブルは作り直さない。
--   - 1 回の評価まとまり（= どの contract / どの model で何回まで呼ぶか）を session として持つ
--   - 予算は run 単位ではなく session 単位。アプリを再起動しても同じ session_key なら累積する
--   - model の pin は session の requested_model が正。jev_contracts.default_model は根拠にしない
-- 起動のたびに再実行されても壊れないこと（CREATE ... IF NOT EXISTS / ALTER は lenient 経由）。

CREATE TABLE IF NOT EXISTS jev_eval_sessions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  -- 呼び出し側が決める識別子。同じキーでの開始は resume を意味する
  session_key TEXT NOT NULL UNIQUE,

  contract_version TEXT NOT NULL
    REFERENCES jev_contracts(contract_version),

  state_schema_version TEXT NOT NULL,
  -- この session で使うモデル。途中で変えない（別モデルは別 session）
  requested_model TEXT NOT NULL,
  -- 開始時に /v1/models から取った ModelCard の canonical JSON
  model_card_json TEXT NOT NULL,

  max_calls INTEGER NOT NULL CHECK (max_calls > 0),
  max_input_tokens INTEGER NOT NULL CHECK (max_input_tokens > 0),

  -- 送信前に予約した論理 call 数。失敗しても戻さない（crash 時に安全側へ倒すため）
  calls_reserved INTEGER NOT NULL DEFAULT 0
    CHECK (calls_reserved >= 0),

  input_tokens_used INTEGER NOT NULL DEFAULT 0
    CHECK (input_tokens_used >= 0),

  output_tokens_used INTEGER NOT NULL DEFAULT 0
    CHECK (output_tokens_used >= 0),

  status TEXT NOT NULL
    CHECK (status IN ('open','exhausted','closed','blocked')),

  -- lifecycle の世代番号。status を動かす操作のたびに +1 する。
  -- open → blocked → open のように文字列が元へ戻っても世代は戻らないので、
  -- 「状態が同じに見えるが別の世代」を compare-and-set で検出できる。
  -- 予約やトークン加算そのものは世代を進めない（status を変えたときだけ）。
  lifecycle_revision INTEGER NOT NULL DEFAULT 0
    CHECK (lifecycle_revision >= 0),

  blocked_reason TEXT,

  created_at TEXT NOT NULL
    DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),

  closed_at TEXT,

  CHECK (calls_reserved <= max_calls)
);

-- 023 で作った call 行に session と request ID を足す。
-- 既存行との互換のため eval_session_id は nullable。C4b 以降が作る新規 call は必ず値を持つ。
ALTER TABLE metadata_match_jev_calls
  ADD COLUMN request_id TEXT;

ALTER TABLE metadata_match_jev_calls
  ADD COLUMN eval_session_id INTEGER
    REFERENCES jev_eval_sessions(id);

CREATE INDEX IF NOT EXISTS idx_jev_calls_session
  ON metadata_match_jev_calls(eval_session_id, created_at);
