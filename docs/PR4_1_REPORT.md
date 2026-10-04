# PR4-1: 通常の診断集計 — 実装報告

状態: **実装・テスト済み。commit していない。** holdout は開いていない。FINAL の昇格指標は NOT_RUN。policy freeze は未定義のまま。
契約: PR4-0 の凍結文書（`docs/PR4_AGGREGATION_CONTRACT.md` ほか）。

## 変更したファイル

| ファイル | 内容 |
|---|---|
| `apps/desktop/src-tauri/src/services/pr4_diagnostics.rs`（新規） | 集計本体（読み取り専用）と 27 件のテスト |
| `apps/desktop/src-tauri/src/services/mod.rs` | `pub mod pr4_diagnostics;` の 1 行 |
| `docs/PR4_1_REPORT.md`（新規）、`docs/PR4_1_CONTRACT_AMENDMENTS.md`（新規）、`docs/PR4_1_FREEZE.txt`（新規） | この報告、PR4-0 契約への amendment、凍結ハッシュ |

migration・既存のコード・matcher / rules / Jev の挙動・DB は変えていない。

## 実装したもの

入口: `pr4_diagnostics::generate_report(conn, cohort, generated_at, head) -> serde_json::Value`
（`generated_at` と `head` は呼び出し側が渡す。関数は時刻も git も見ない。同じ入力から同じレポートが出る）。
cohort は明示的に選ぶ: `DevelopmentAudit`（A1）/ `ProductionObserved`（A2）。
UI・command・手動ランナーは足していない（backend + テストだけ）。

- 判定器別（rules-safe / rules-tags-shadow）の **理由コード別件数と率**（14 コードすべてを 0 件でも出す。NO_REASON_AUTO / EMPTY_REASONS_NON_AUTO / UNKNOWN_OR_NULL の bucket を別に持つ）
- **REVIEW** の件数と率 / **AUTO** の件数と coverage / **abstention**（REVIEW + UNRESOLVED）の件数と率。分母は `eligible_decision_count`
- **rules-safe と rules-tags-shadow の比較**（同じ `run_id` の 2 行だけを組にする）: decision / top identity / AUTO の結果（5 区分の内訳つき）/ 理由集合（集合の等しさ）。各次元に `comparable_count` / `agreement_count` / `disagreement_count` / `not_comparable_count`（理由別）/ `agreement_rate`（`comparable_count > 0` のときだけ）
- `diagnostic_gt` は常に `NOT_IMPLEMENTED_PR4_1`、`final_promotion` は常に `NOT_RUN`

レポートの版: **`pr4-diagnostic-report-1`**。トップレベル: `report_metadata` / `reason_codes` / `review` / `auto` / `abstention` / `profile_comparison` / `diagnostic_gt` / `final_promotion`。
`reason_codes` / `review` / `auto` / `abstention` は matcher ごとに入れ子（指示の例は単一の値だが、2 つの matcher を混ぜないため）。

## 守ったこと（コードとテストの対応）

| 条件 | 実装 | テスト |
|---|---|---|
| verdicts が正（`runs.decision` は適格性の判定だけ） | 集計は `metadata_match_verdicts` から | `review_auto_and_abstention_…` ほか |
| shadow run を source として数えない | `mode='shadow'` → `shadow_duplicate_run` | `shadow_mode_runs_are_not_counted_as_source_runs` |
| audit run は `review_tasks` で識別 | `audit_sample` task の `run_id` | `audit_and_production_runs_are_separated_by_the_review_task_link` |
| holdout は集計の前に除く・中身を読まない | 判定は runs / review_tasks のメタデータだけ。verdict は適格な run の分だけ読む | `a_holdout_row_never_reaches_aggregation_and_its_content_is_not_exposed`、`the_report_does_not_read_labels_at_all`（labels テーブルを消しても動く） |
| 履歴の版を混ぜない | 現行以外の版は `historical_version_ambiguous` / `unsupported_profile_version` | `ambiguous_historical_and_unsupported_versions_are_bucketed_not_merged` |
| 何も黙って落とさない | すべての run が「適格」か「ちょうど 1 つの除外理由」。除外理由は 15 個すべてを毎回出す | `assert_partition`（各テストで検証） |
| 比較不能を捨てない | `not_comparable_by_reason` | `missing_counterparts_…`、`different_candidate_sets_…` |
| 書き込みなし | 集計中は `PRAGMA query_only = ON`（終了後に元へ戻す）。モジュールに書き込み文なし | `the_aggregation_writes_nothing_…`、`query_only_blocks_writes_…`、`the_module_has_no_write_statements` |

除外理由（優先順位順）: `sealed_holdout` / `unsupported_sampling_metadata` / `shadow_duplicate_run` / `unsupported_run_mode` / `replay_run` / `run_error` / `run_skipped` / `unsupported_run_state` / `audit_run_other_cohort` / `production_run_other_cohort` / `audit_task_not_valid` / `unsupported_audit_task_state` / `live_evidence_policy_ambiguous` / `historical_version_ambiguous` / `unsupported_profile_version`。

holdout の境界は `metadata_review_tasks` の抽出記録だけから導く: `purpose='holdout'`、または `cohort='live'`（live + development は禁止の組み合わせなので holdout 側に倒す）。holdout の run と、同じ作品の他の run を除く。抽出記録が読めない・足りない task の作品の run も除く（holdout でないと言い切れないため）。

## 指示と PR4-0 契約との差、判断した点（承認済み。正式な記録は `docs/PR4_1_CONTRACT_AMENDMENTS.md`）

1. **top identity で両方 NULL（候補なし）の扱い。** PR4-0 契約は「両方 NULL は一致として数えるが別掲」。PR4-1 の指示は「両側が identity を持つときだけ比較」。**指示に従い、両方 NULL は `not_comparable(both_no_identity)` として数えた。** レポートに `deviation_note` を残している。
2. **`final_promotion.reason`** は指示どおり `HOLDOUT_SEALED_POLICY_NOT_FROZEN`（PR4-0 契約の JSON は `HOLDOUT_SEALED / POLICY_NOT_FROZEN`）。
3. **A2（production）の `evidence_class='live'` は除外**（`live_evidence_policy_ambiguous`）。PR4-0 は「別 bucket・合算しない」。holdout の所属が policy freeze まで未定義なので、より保守的に除外して件数を出す形にした。A2 の件数は、live の run が多い DB では小さくなる。
4. **rules-1 は指標に使わない。** 凍結の前後で `matcher_version` が同じ文字列で、より強い境界（不変な永続フィールド）は見つからなかった。適格な run のうち rules-1 の verdict 行がある件数だけを `rules_1_version_boundary`（`status = historical_version_boundary_unresolved`）として報告する。
5. 抽出記録の `purpose` / `cohort` は DB では保護されていない（PR4-0 の指摘のとおり）。このモジュールは読んで判定するだけで、改変の検出（整合検査）はしない。**すべての report の `report_metadata` に `cohort_boundary_integrity = APPLICATION_VALIDATED_NOT_DB_ENFORCED` を入れる。**
6. 上記 1〜3 と 5 は PR4-1 側の amendment として固定した（PR4-0 の凍結文書は書き換えていない）。除外理由 `live_holdout_overlap_unresolved` は `live_evidence_policy_ambiguous` に改名した（一時的な安全側の処理で、live を恒久的に対象外とする意味ではない）。

## ブロッカーは無かった

PR4-0 で BLOCKED とした GT の意味（`label='none'`、統合 rank、`manual_*`）には触れていない。holdout の区別に、メモリ上の ID などは使っていない。

## テスト

| 実行 | 結果 |
|---|---|
| `cargo test --lib pr4_diagnostics`（新規 27 件） | 27 passed / 0 failed |
| `cargo test --lib`（全件の回帰。既存のテストを含む） | 465 passed / 0 failed / 8 ignored（もともと手動用の `#[ignore]`） |
| `cargo check --lib`（通常ビルド） | 通過（このモジュールの警告なし） |

テストの範囲（新規 27 件）: 理由コード（0 件・1 件・複数・NULL / 未知・率の合計が 1 超）/ REVIEW・AUTO・abstention（分母・各状態・分母 0）/ 比較（完全一致・各不一致・safe 欠落・shadow 欠落・候補集合の違い・重複 verdict の不可能性・理由集合の順序違い・未対応の matcher 版）/ cohort（holdout・shadow 複製・audit・production・live・版の曖昧さ・メタデータ欠落・task の state・replay / error / skipped）/ レポートの安全性（NOT_RUN・precision / recall の語が無い・holdout の中身が出ない・除外理由が毎回出る・書き込みなし・スキーマ不足の報告・800 件超の分割読み）。

## 既知の限界

- アプリ本体からはまだ呼ばれない（呼び出し側 = 手動ランナーや command は後続の gate）。そのため、実データでの件数は未確認。
- rules-safe と rules-tags-shadow は入力の候補集合が違う。比較は、その前提を踏まえて読む。
- 版の境界は version 文字列でしか分からない。
