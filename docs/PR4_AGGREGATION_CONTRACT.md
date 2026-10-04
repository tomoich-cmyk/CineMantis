# PR4-0: 集計契約と指標の境界（設計のみ）

状態: **設計・棚卸しだけ。実装は無い。** holdout は開いていない。DB の行は読んでいない。昇格判断は行っていない。
前提: C5c.5 は FINAL CLOSED（`docs/C5C5_CLOSEOUT.md`）。リポジトリ HEAD `63af144`。
機械可読版: `docs/pr4_schema_inventory.json`（フィールド棚卸し 47 行）、`docs/pr4_metric_contract.json`（指標契約）。
意味をコードから確かめられなかったものは **UNKNOWN** と書く。推測で埋めない。

## 1. 棚卸しで確定したこと（コードの根拠つき）

詳細は `pr4_schema_inventory.json`。集計の設計に効くものだけを挙げる。

1. **判定器ごとの結論は `metadata_match_verdicts` が正。** `runs.decision` は rules-safe の複製（error_text があれば ERROR で上書き）。
   - rules-safe / rules-tags-shadow: 理由が空なら AUTO、候補なしなら UNRESOLVED（`NO_CANDIDATES`）、それ以外は REVIEW。
   - rules-1: 候補があれば AUTO、無ければ UNRESOLVED。**REVIEW は出ない。** `reasons_json` は理由コードではなく notes。
   - Jev（`jev-policy`）: **AUTO は書かない**（REVIEW / UNRESOLVED のみ）。
2. **verdict の `tmdb_id` / `media_type` は REVIEW でも入る**（その判定器の top candidate。AUTO のときだけ適用される）。
3. **rules-safe と rules-tags-shadow は入力の候補集合が違う。** rules-safe は旧経路の候補だけ（`legacy_ranked`）、rules-tags-shadow は統合候補（`combined_ranked`）。保存される候補は統合後。`candidates.rules_rank` / `rules_score` は **統合集合上の値で、判定器固有ではない。**
4. **判定器別の候補 rank を永続化する production の経路は無い。** `metadata_match_candidate_scores` に書くのは (a) `db.rs` の `backfill_legacy_candidate_scores`（旧経路だけで見つかった候補の score のみ、rank は NULL）と (b) `jev_shadow.rs`（jev-call）だけ。rules-safe / rules-tags-shadow の rank は取れない。
5. **`mode='shadow'` の run は safe run の複製**（decision・理由・候補・決定的な verdict を写す）。run 件数に混ぜると二重に数える。除外が必須。
6. **audit run は run 単体では production run と区別できない。** 区別は `metadata_review_tasks`（`reason='audit_sample'`、`run_id` で繋がる、`sampling_json.purpose`）で行う。
7. **`evidence_class` は run の種別ではない。** 作品のファイルの original 取得状況から導出した分類（live / reconstructed_clean / historical_audit_only）。
8. GT ラベルは `review_task_id` と `run_id`（= 人が見た audit run）に紐づく strong ラベル（`review_confirm` / `review_pick_other` / `review_none`）。作品ごとの有効ラベルは 1 件（`superseded_at IS NULL`）。
9. **版が混ざっている。** 履歴は 2026-09-23 の数時間に集中している: `rules-safe-1` / `rules-tags-shadow-1` → `-2`（`af63b4d`）、rules-1 の凍結（`91b533d`）。rules-1 の `matcher_version` は凍結後も `rules-1` のままで、**凍結前後の行を version 文字列で区別できない。**
10. 非テストの書き込み元が見つからない値: `mode='gated'`、`governing_matcher='policy'`、`decision='SKIPPED'`、verdict の `jev-choice-shuffled` / `jev-isolated`、review reason の `matcher_disagreement` / `user_initiated`。意味は CHECK 以上には UNKNOWN。

## 2. cohort の用語（混ぜない）

| 型 | 定義 | 使える指標 |
|---|---|---|
| **A1 development_audit** | `audit_sample` かつ `sampling_json.purpose='development'` の task に繋がる audit run（`mode=safe`, `trigger_kind=batch`, `batch_id=sample_id`）と、その有効な strong ラベル | 分布指標 + 診断 GT 指標 |
| **A2 production_observed** | `mode='safe'`・audit task に繋がらない・`trigger_kind<>'replay'` の run。ラベルなし | 分布指標だけ |
| **B sealed holdout** | `sampling_json.purpose='holdout'`（live）。PR4-0 と通常の PR4 report では **読まない・集計しない** | FINAL 評価（policy freeze 後に 1 回だけ） |
| **C historical / incompatible** | version / schema / verdict 行が直接比較できない行 | 別 bucket で報告。A1 / A2 に混ぜない |

- A2 の `evidence_class='live'` は、holdout との重なりが **UNKNOWN**（holdout の所属定義が未確定）。**policy freeze で定義されるまで、live の bucket は他と合算せず、調整・昇格に使わない**旨を report に明記する。
- 通常集計の SQL は holdout の task / run / label を除く条件を持ち、将来のテストでそれを検証する。

**C に落とす行**: `policy_version` が現行でない / rules-tags-shadow の version が現行でない・verdict 行が無い（022 より前）/ `query_source IS NULL`（022 より前）/ rules-1 の verdict が凍結前に作られた可能性。backfill はしない。

すべての report が持つもの: cohort 型、matcher / profile の version、policy / schema の version、run の選択境界（SQL 条件・run_id の最小最大・件数）、`eligible_rows`、`excluded_rows`、除外理由別の件数。

## 3. 通常の診断指標（契約）

適格 run（`eligible_rows`）の除外（理由別に件数を出す）: `mode='shadow'` / `trigger_kind='replay'` / `decision IN ('ERROR','SKIPPED')` / holdout 所属（または所属不明）/ cohort C。**`applied` は除外理由ではない**（結果であって適格性ではない）。matcher ごとの指標は、その matcher の verdict 行がある run だけを分母にする。行が無い run は「比較不能」として別計上する。

### 3.1 理由コード別件数 — 形は凍結
- 対象は `verdicts.reasons_json`（rules-safe と rules-tags-shadow）。`decision_reasons_json` は複製なので使わない。rules-1 は notes なので含めない。
- 1 行に複数のコードがありうる（multi-label）。`count` = そのコードを含む行数、`rate` = `count / eligible_rows`（合計は 1 を超えうる）。
- 空配列は `NO_REASON_AUTO` bucket、NULL・配列でない・未知のコードは `UNKNOWN_OR_NULL` bucket。**黙って落とさない。**
- 「主理由」は定義しない（コードに優先順位が無い = UNKNOWN）。

### 3.2 REVIEW rate — 形は凍結
`review_count = verdict.decision='REVIEW'`、`review_rate = review_count / eligible_rows`（matcher ごと）。REVIEW に当たる永続値は `'REVIEW'`。review task（`review_decision` / `legacy_pending`）は REVIEW run から作られる別の概念で、件数だけ別掲し、REVIEW rate と同一視しない。

### 3.3 rules-safe と rules-tags-shadow の一致 — 形は凍結（4 つに分けて持つ）
比較可能 = 同じ `run_id` に両方の verdict 行がある（かつ cohort C でない）。片方しか無い run は `not_comparable` として件数を出す。
- `decision_agreement`: decision が同じ。
- `top_identity_agreement`: `(tmdb_id, media_type)` が同じ（両方 NULL は一致として別掲）。
- `auto_outcome`: `{both_auto_same_identity, both_auto_different_identity, rules_safe_auto_only, shadow_auto_only, neither_auto}`。
- `reason_set_agreement`: 理由コード集合の完全一致。

**1 つの「一致率」に潰さない。** 入力の候補集合が違う（§1-3）ことを前提に読む。一致率を昇格判断に使わない。

### 3.4 AUTO coverage — 形は凍結
`auto_count / eligible_rows`（matcher ごと）。根拠は `audit_run.rs` の集計定義コメント（AUTO / 全件）。FINAL では分母が holdout 全件だが、診断では cohort の適格 run 全件に置き換え、分母の定義を report に明記する。

### 3.5 abstention rate — 形は凍結
`(REVIEW + UNRESOLVED) / eligible_rows`（同コメントの定義）。REVIEW と同一視しない。ERROR / SKIPPED は abstention ではなく除外として別計上。GT 側の人手の defer、Jev の「決定なし」は別の概念で、入れない。decision は 3 値なので同一 `eligible_rows` 上では `auto_coverage + abstention_rate = 1`。冗長だが両方出し、一致を report 内で検査する。

### 3.6 `diagnostic_conditional_auto_precision` — 一部が BLOCKED
cohort は **A1 だけ**。名前は必ずこれ。FINAL precision とは呼ばない。
- **評価可能な GT**: task の `details_json.state='resolved'` かつ `label.review_task_id = task.id` かつ `label.run_id = task.run_id` かつ `strength='strong'` かつ method が `review_confirm` / `review_pick_other` / `review_none` かつ `superseded_at IS NULL`。
- **未解決 GT（報告して除外）**: state が `deferred` / `ready` / `selected` / `stale` / `generation_error`、およびラベルが上書きされた task（理由別の件数）。
- **部分集合**: `label='tmdb'` で、ラベルの `(tmdb_id, media_type)` が候補の `rules_rank<=3` に居る run（コードの定義「GT が top3 に居た部分集合」）。top-3 は**統合集合上の rank**（§1-4）。
- **分子**: 部分集合のうち、その matcher の verdict が AUTO で top がラベルと一致する件数。**分母**: 部分集合のうち、その matcher の verdict が AUTO の件数。
- report に出すもの: 分子・分母・`evaluable_gt_count`・`unresolved_gt_count`（state 別）・`excluded_count`（理由別）・`subset_size`。
- **BLOCKED（policy freeze で決める）**: (a) `label='none'` の扱い（コードに定義なし。件数だけ別掲し precision に入れない）、(b) 統合 rank を top-3 の基準にしてよいか、(c) production run に紐づく `manual_*` の strong ラベルを GT に使うか（UNKNOWN。PR4-0 では使わない）。

## 4. FINAL の昇格指標の境界（計算しない）

- 指標: **FINAL end-to-end AUTO precision**。コード上の定義: 「正しい AUTO / すべての AUTO（holdout 全体）」。candidate set の外に正解があるのに別候補を AUTO した場合も誤り 1 件（分子から外し分母に残す）。AUTO coverage と abstention rate を**併記**する。
- 原則: holdout だけを使う / policy freeze の後に 1 回だけ開く / development の診断指標を昇格ロジックの入力にしない / **通常 report と別の実行経路（別の関数・別の入口）にする** / 未解決・abstain を、将来の policy 仕様が明示しない限り分母から黙って外さない / 昇格に使えるのはこの指標だけ。
- 分母の意味（`label='none'`、GT 未解決、abstain の扱い）は **BLOCKED BY POLICY FREEZE。PR4-0 では発明しない。**
- 通常の PR4 report の `final_promotion` は常に `{"status":"NOT_RUN","reason":"HOLDOUT_SEALED / POLICY_NOT_FROZEN"}`。development のデータで埋めない。

## 5. 履歴データの互換性（棚卸しの結論）

| 比較 | 状況 |
|---|---|
| rules-1 / rules-safe | verdict は 021 以降の全 run にある。ただし rules-1 は凍結（`91b533d`）の前後で version 文字列が同じ。rules-safe は `-1` と `-2` が混在しうる |
| rules-tags-shadow | verdict の CHECK に追加されたのは 022 で、`persist_run` は `Option` で書く。022 より前の run には行が無い |
| legacy（candidate_scores） | 023 で導入。`db.rs` が旧経路だけの候補の score を後埋め（rank は NULL）。他の matcher の後埋めは無い |
| jev-call | 023〜025 以降、sidecar が処理した run の兄弟 shadow run にだけある。AUTO を出さないので precision の入力にならない |

- backfill は PR4-0 では行わない。集計は旧行を除外するか別 bucket にする（§2 の C）。
- 実際の行数・各版の run 数は**未確認**（DB を読んでいない）。実装時に、holdout を除いた条件で件数だけ確認する。

## 6. 通常 report のスキーマ（将来）

`pr4_metric_contract.json` の `report_schema` を正とする。トップレベル: `report_metadata` / `reason_codes` / `review` / `profile_comparison` / `auto` / `diagnostic_gt` / `final_promotion`。

## 7. 未解決・ブロックされている項目

`docs/PR4_POLICY_FREEZE_CHECKLIST.md` を参照。要点: policy freeze が未定義 / holdout の所属と開封手順が未定義 / FINAL の分母が未定義 / どの判定器を昇格対象にするか未指定 / 候補生成の実行経路が未凍結。
