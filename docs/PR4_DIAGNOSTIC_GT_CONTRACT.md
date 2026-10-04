# PR4-2: 診断用 GT の意味契約（`diagnostic_conditional_auto_precision`）

状態: **契約だけ。実装は無い。** holdout は開いていない。DB は読んでいない。FINAL の昇格指標は NOT_RUN。policy freeze は未定義。
機械可読版: `docs/pr4_gt_semantics.json`（`contract_version = pr4-gt-contract-1`）。
PR4-0 の凍結ファイルは書き換えない。この文書は、PR4-0 で BLOCKED だった 3 点を決着させる amendment 層。

## 1. 指標

**`diagnostic_conditional_auto_precision`** — 開発・診断専用。次のものではない: FINAL end-to-end AUTO precision / 昇格指標 / recall@1 / recall@3。

- **評価単位**: audit_sample の review task = そのラベルが指す audit run（`label.run_id = task.run_id`）。作品単位ではない。
- **分母**: 適格な評価単位（PR4-1 の A1 cohort）のうち、(1) その matcher の verdict が **AUTO**、かつ (2) **信頼できる・解決済み・評価可能な GT** がある単位の数。matcher（rules-safe / rules-tags-shadow）ごとに数える。
- **分子**: 分母のうち、positive GT かつ AUTO が選んだ `(media_type, tmdb_id)` が GT と**完全一致**する単位の数。
- `precision = auto_correct_count / auto_evaluable_count`。分母が 0 のときは **`null`**（NaN・無限大・0 にしない）。
- 機械側の出力は `metadata_match_verdicts`（PR4-1 と同じ）。`runs.decision` は権威ではない。REVIEW / UNRESOLVED は分母に入らない（AUTO coverage / REVIEW rate / abstention rate には残る。coverage と precision を混ぜない）。

## 2. `none` の扱い（決定）

信頼できる strong の `review_none`（「対応する TMDB 作品は存在しない」）は、**評価可能な negative GT**。

| GT | 機械の状態 | 結果 |
|---|---|---|
| positive | AUTO・同じ identity | 正しい |
| positive | AUTO・別の identity | 誤り |
| **none** | **AUTO（何かを選んだ）** | **誤り。分母に入る** |
| defer / 未解決 | 何でも | 分母から除外（`unresolved_gt_count`） |

- `none` を「GT なし」として除外しない。確定した none は negative GT で、AUTO が何かを自動採用したなら誤りとして precision に効く。
- TMDB-positive 専用の指標（candidate recall など）では、従来どおり none は分母外。両者を混ぜない。
- `none` を次から推測しない: 候補 0 件 / 検索結果 0 件 / UNRESOLVED / defer / ラベルが無いこと。明示的に信頼できる GT の判断だけ。

## 3. `defer` と未解決

`defer` は**ラベルの値ではなく、課題の state**（`details_json.state='deferred'`）。未解決として、正しくも誤りでもなく分母から外し、state 別に `unresolved_gt_count` へ出す（`ready` / `selected` / `stale` / `generation_error` も同様）。**none に変換しない。**

## 4. 完全一致のみ・top-3 は使わない（決定）

- 正しさは **`(media_type, tmdb_id)` の完全一致だけ**。部分点なし。title の類似・rank の近さ・top-3 に居たこと・score・Jev・理由コード・同一シリーズは使わない。
- **統合集合の rank は precision に使わない。** rules-safe は旧経路の候補集合、rules-tags-shadow は統合候補集合を入力にし、永続化された `rules_rank` は統合集合上の値。matcher 別の rank は永続化されていない。
- `top3_for_auto_precision = "NOT_APPLICABLE"`。将来 `diagnostic_candidate_recall_at_3` を作るなら**別の指標**で、同じ候補集合（profile）に属する rank だと証明できる場合だけ計算する。rules-safe が統合 `rules_rank` を流用することは禁止。

## 5. 信頼できる GT の出どころ（allowlist）

manual だから GT、最新だから GT、とは扱わない。`review_*` の prefix 一致も使わない。**次の条件をすべて満たすラベルだけ**:

1. `method` が `review_confirm` / `review_pick_other` / `review_none` のいずれか（3 値を明示列挙）
2. `strength = 'strong'`
3. `review_task_id` の課題が `reason='audit_sample'` で、`task.work_id = label.work_id`、`task.run_id = label.run_id`（NULL でない）
4. `task.resolved_at` が入っていて、`task.resolution_label_id = label.id`
5. `details_json`: `state='resolved'`、`review_protocol_version='gt-review-1'`、`resolution_method = label.method`
6. `review_pick_other` は `details_json.tmdb_verified = true` が必須（TMDB での存在確認の記録）
7. `sampling_json`: `protocol_version='gt-protocol-1'`、`purpose='development'`、`sample_id` が **`c5c5c-redo-01` 〜 `c5c5c-redo-05`**（committed の `docs/C5C5_CLOSEOUT.md` が定める開発用 50 task）
8. `superseded_at IS NULL`

- 旧 provisional の `c5c5-dev-expand-01`（出所未確定）は、構造的には review_* のラベルに見えても allowlist に入らない（`gt_sample_not_allowlisted`）。task ID は使わない。
- **production の `manual_apply` / `manual_direct_id` は信頼しない**（`untrusted_manual_gt_source`）。どの UI / protocol で作られたか、blind だったか、候補集合が何だったか、後から変わっていないかを、永続化されたデータから証明できないため。`lock`（weak）と `bulk_accept`（weak・非テストの書き込み元なし）も GT にしない。
- allowlist への追加は、別の GT 群が正式な protocol 由来だと証明できたときに、別の amendment で行う。

## 6. 実在するラベル値の棚卸し

| 値（label / method） | strength | 意味 | 同一性の要否 | 信頼 | precision に使う | 使わない理由 |
|---|---|---|---|---|---|---|
| tmdb / `review_confirm` | strong | POSITIVE（run の候補行から identity を読む） | 要 | allowlist を満たせば yes | 条件つき yes | `gt_sample_not_allowlisted` / `gt_provenance_incomplete` / `gt_superseded` |
| tmdb / `review_pick_other` | strong | POSITIVE（候補集合の外・存在確認済み）。AUTO は常に誤り | 要 | allowlist + `tmdb_verified` | 条件つき yes | 同上 |
| none / `review_none` | strong | NONE（identity は NULL） | 不要 | allowlist を満たせば yes | 条件つき yes（negative GT） | 同上 |
| tmdb\|none / `manual_apply` | strong | UNKNOWN（出どころが不明） | tmdb のとき要 | **no** | no | `untrusted_manual_gt_source` |
| tmdb\|none / `manual_direct_id` | strong | UNKNOWN | 同上 | **no** | no | `untrusted_manual_gt_source` |
| tmdb\|none / `lock` | weak | UNKNOWN（固定の操作。中身を確かめたとは限らない） | 同上 | no | no | `weak_label_not_gt` |
| tmdb\|none / `bulk_accept` | weak | UNKNOWN（書き込み元なし） | 同上 | no | no | `weak_label_not_gt` |
| 上記以外の method | n/a | UNKNOWN | n/a | no | no | `unknown_label_semantics` |
| ラベルなし（課題の state） | n/a | DEFER / UNRESOLVED | n/a | n/a | no | `unresolved_gt` |

意味はコードとスキーマ（021 の CHECK、`gt_review.rs` の `resolve`、`match_history.rs` の `record_label` と `LabelMethod`）から確かめたもの。名前から推測していない。

## 7. 複数ラベル・衝突

- **「最新のラベルを使う」はしない。**
- 同じ評価単位（同じ `review_task_id`）で内容が同一の信頼できる GT → 1 件に畳む。内容が食い違う → `gt_conflict` として除外。
- 信頼できる + 信頼できない → 信頼できるものを使い、信頼できないものは除外として記録する。
- resolved + defer → resolved を自動で優先しない（置き換えの意味が protocol で確立されていない）。課題が resolved ならラベルがあり、deferred ではない。
- **後続のラベルで上書きされた信頼できるラベル → `gt_superseded` として除外。** production の `record_label` は active なラベルを（strong も）superseded にするが、それが GT の誤りの発見なのか別の操作なのかを protocol が定めていない。保守的に使わない。
- 1 つの作品に信頼できる評価単位が複数ある → `gt_multiple_units_per_work` として全て除外。

## 8. cohort の境界・FINAL との分離

- PR4-1 の A1 cohort の選択と除外をそのまま再利用する。holdout は封印のまま: 照会しない、PR4-2 の意味の検証にも使わない、ラベル / verdict / 候補 / 件数を寄与させない。holdout の評価単位は GT を組み立てる**前**に除く。`cohort_boundary_integrity = APPLICATION_VALIDATED_NOT_DB_ENFORCED` を保つ。
- **PR4-2 の GT の意味は FINAL の意味ではない。** `final_promotion.status = NOT_RUN`（`HOLDOUT_SEALED_POLICY_NOT_FROZEN`）。FINAL の分母と abstention の扱いは policy freeze まで BLOCKED のまま。

## 9. 将来の `diagnostic_gt` セクション（PR4-3 で実装）

`status`（IMPLEMENTABLE / BLOCKED）/ `gt_source_contract_version` / `positive_gt_count` / `none_gt_count` / `unresolved_gt_count`（state 別）/ `excluded_gt`（8 つの理由）/ `excluded_holdout_count` / matcher ごとの `auto_evaluable_count`・`auto_correct_count`・`auto_incorrect_count`・`not_in_denominator`（`machine_not_auto` / `auto_without_identity` / `machine_verdict_missing`）・`diagnostic_conditional_auto_precision`（number | null）/ `denominator_definition` / `correctness_definition = exact_media_type_tmdb_id` / `top3 = NOT_APPLICABLE`。PR4-2 は本番 DB からこれらの件数を計算しない。

## 10. 既知の限界（PR4-3 の前に認識しておくこと）

1. **D / E / F の段階で解決した GT（71, 93, 96, 104 など）は、DB のラベルになっていない。** 監査ディレクトリの判断であり、`docs/C5C5_CLOSEOUT.md` も「DB には触れていない」と記録している。DB だけから診断 precision を作ると、それらは未解決として数えられる。取り込みの protocol は未定義で、別の amendment が要る。
2. allowlist の sample は `docs/C5C5_CLOSEOUT.md` に由来する。
3. allowlist の sample に信頼できる none が実際に何件あるかは未確認（DB を読んでいない）。
4. rules-safe と rules-tags-shadow は入力の候補集合が違うので、matcher 間の precision の比較は、その違いを前提に読む。
