# PR4-1: PR4-0 契約への amendment（承認済み）

PR4-0 の凍結文書（`docs/PR4_AGGREGATION_CONTRACT.md`、`docs/pr4_metric_contract.json` ほか）は **書き換えない**。
PR4-1 の実装で生じた次の差分を、PR4-1 側の amendment として固定する。PR4-0 と食い違う場合は、PR4-1 の report（`pr4-diagnostic-report-1`）についてはこの文書が優先する。

## A1. top identity で両側とも identity が無い場合

- PR4-0: 両方 NULL（候補なし）は一致として数えるが別掲。
- **PR4-1（採用）: `not_comparable`（理由 `both_no_identity`）として数える。** 一致にも不一致にも入れない。
- 理由: 候補の identity が両側とも存在しない状態を「identity が一致した」と呼ぶより、比較不能とするほうが指標の名前（top identity agreement）と整合する。
- レポートの `profile_comparison.top_identity.deviation_note` に残す。

## A2. `final_promotion.reason` の正規化

- PR4-0: `HOLDOUT_SEALED / POLICY_NOT_FROZEN`（2 つの条件の併記）。
- **PR4-1（採用）: `HOLDOUT_SEALED_POLICY_NOT_FROZEN`**（1 つの machine-readable な code）。
- 意味は変わらない（holdout が封印されている、かつ policy freeze が未定義）。`final_promotion.status` は常に `NOT_RUN`。

## A3. A2 の `evidence_class='live'`

- PR4-0: 別 bucket にして合算しない（holdout との重なりが UNKNOWN）。
- **PR4-1（採用）: 現時点では eligible にせず、明示的な除外理由 `live_evidence_policy_ambiguous` として件数を出す。**
- これは **一時的な安全側の処理**で、policy freeze の後に定義が変わりうる。live を恒久的に A2 の対象外とする意味ではない。
- 除外理由の名前は、当初の `live_holdout_overlap_unresolved` から `live_evidence_policy_ambiguous` に改めた（意味は「live の扱いを決める policy が未確定」）。

## A4. cohort 境界の信頼性の注記（追加）

- PR4-0 の棚卸し: `sampling_json`（`purpose` / `cohort`）は DB の制約・トリガーでは保護されていない。
- **PR4-1（採用）: すべての report の `report_metadata` に次を入れる。**
  - `cohort_boundary_integrity = "APPLICATION_VALIDATED_NOT_DB_ENFORCED"`
  - `cohort_boundary_note`（アプリケーションが抽出記録を検証して判定しており、DB で不変が保証されているわけではない、という注記）
- これは集計を無効にするものではなく、監査上の注記。DB 側の保護、または開封前の整合検査をどうするかは policy freeze の前提（`PR4_POLICY_FREEZE_CHECKLIST.md` の項目 20）のまま残る。

## 変えていないもの

PR4-0 の指標の形（理由コード、REVIEW rate、AUTO coverage、abstention、4 つの一致の次元）、分母の定義、FINAL の境界（NOT_RUN）、BLOCKED の GT の意味（`label='none'`、統合 rank、`manual_*`）。これらは PR4-2 以降。
